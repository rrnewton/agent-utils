//! Command-line interface for `herdr-run`.

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

const SUBCOMMANDS: [&str; 5] = ["check", "doctor", "config", "target", "userguide"];

#[derive(Clone, Debug, Default, PartialEq)]
struct Args {
    positional: Vec<String>,
    config: Option<PathBuf>,
    agent: Option<String>,
    cwd: Option<String>,
    timeout: Option<f64>,
    wait_ready: Option<f64>,
    no_cache: bool,
    json: bool,
    dry_run: bool,
    userguide: bool,
}

enum ParseResult {
    Args(Args),
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
    let args = match parsed {
        ParseResult::Args(args) => args,
        ParseResult::Exit(code) => return code,
    };
    if args.userguide
        || args
            .positional
            .first()
            .is_some_and(|value| value == "userguide")
    {
        print!("{}", crate::USER_GUIDE);
        return 0;
    }
    match run(args) {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(io::stderr(), "herdr-run: {error}");
            error.exit_code()
        }
    }
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
    let mut args = Args::default();
    let mut index = 0;
    let mut options = true;
    while index < raw.len() {
        let token = &raw[index];
        if options && token == "--" {
            options = false;
            index += 1;
            continue;
        }
        if options {
            match token.as_str() {
                "--help" | "-h" => {
                    print_help();
                    return Ok(ParseResult::Exit(0));
                }
                "--version" => {
                    println!("herdr-run {}", env!("CARGO_PKG_VERSION"));
                    return Ok(ParseResult::Exit(0));
                }
                "--no-cache" => args.no_cache = true,
                "--json" => args.json = true,
                "--dry-run" => args.dry_run = true,
                "--userguide" => args.userguide = true,
                "--config" | "--agent" | "--cwd" | "--timeout" | "--wait-ready" => {
                    let value = raw
                        .get(index + 1)
                        .ok_or_else(|| format!("argument {token}: expected one value"))?
                        .clone();
                    if value.starts_with('-') && value != "-" {
                        return Err(format!("argument {token}: expected one value"));
                    }
                    assign_value(&mut args, token, value)?;
                    index += 1;
                }
                _ if token.starts_with("--config=")
                    || token.starts_with("--agent=")
                    || token.starts_with("--cwd=")
                    || token.starts_with("--timeout=")
                    || token.starts_with("--wait-ready=") =>
                {
                    let (option, value) = token
                        .split_once('=')
                        .expect("the guarded option contains equals");
                    assign_value(&mut args, option, value.to_owned())?;
                }
                _ if token.starts_with('-') => {
                    return Err(format!("unrecognized arguments: {token}"))
                }
                _ => args.positional.push(token.clone()),
            }
        } else {
            args.positional.push(token.clone());
        }
        index += 1;
    }
    Ok(ParseResult::Args(args))
}

fn assign_value(args: &mut Args, option: &str, value: String) -> std::result::Result<(), String> {
    match option {
        "--config" => args.config = Some(PathBuf::from(value)),
        "--agent" => args.agent = Some(value),
        "--cwd" => args.cwd = Some(value),
        "--timeout" => args.timeout = Some(parse_seconds(option, &value)?),
        "--wait-ready" => args.wait_ready = Some(parse_seconds(option, &value)?),
        _ => return Err(format!("unsupported option {option}")),
    }
    Ok(())
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

fn run(mut args: Args) -> Result<i32> {
    let mut positional = std::mem::take(&mut args.positional);
    let subcommand = positional
        .first()
        .filter(|value| SUBCOMMANDS.contains(&value.as_str()))
        .cloned();
    if subcommand.is_some() {
        positional.remove(0);
    }
    let start = std::env::current_dir().map_err(|error| {
        HerdrRunError::config(format!("cannot determine current directory: {error}"))
    })?;
    let config = load_config(args.config.as_deref(), &start)?;

    let explicit_agent = args.agent.as_deref().unwrap_or("");
    let (agent, command) = if positional.len() == 2 && explicit_agent.is_empty() {
        (positional[0].clone(), Some(positional[1].clone()))
    } else if positional.len() == 1 {
        (
            if explicit_agent.is_empty() {
                default_agent()
            } else {
                explicit_agent.to_owned()
            },
            Some(positional[0].clone()),
        )
    } else if positional.len() > 1 {
        let agent = if explicit_agent.is_empty() {
            "<agent>"
        } else {
            explicit_agent
        };
        let joined = positional.join(" ");
        let suggested = if explicit_agent.is_empty() {
            positional[1..].join(" ")
        } else {
            joined.clone()
        };
        eprintln!(
            "herdr-run: pass the command as ONE quoted argument, not as separate words.\n  you wrote:  herdr-run {joined}\n  instead:    herdr-run --agent {agent} '{suggested}'\nRe-joining loose words would silently change the quoting of your arguments."
        );
        return Ok(2);
    } else {
        (
            if explicit_agent.is_empty() {
                default_agent()
            } else {
                explicit_agent.to_owned()
            },
            None,
        )
    };

    match subcommand.as_deref() {
        Some("config") => command_config(&config, &agent),
        Some("target") => command_target(&config, &agent, !args.no_cache),
        Some("doctor") => command_doctor(&config, &agent),
        Some("check") => {
            if let Some(command) = command {
                command_check(&config, &command)
            } else {
                eprintln!("herdr-run check: needs a command to check");
                Ok(2)
            }
        }
        Some("userguide") => Ok(0),
        Some(_) => unreachable!(),
        None => match command {
            Some(command) => run_command(&config, &agent, &command, &args),
            None => {
                print_help();
                println!("\nNothing to do: no command given. See 'herdr-run userguide'.");
                Ok(0)
            }
        },
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

fn command_check(config: &Config, command: &str) -> Result<i32> {
    match admit(command, config) {
        Ok(admission) => {
            println!(
                "ALLOWED: program={} subcommand={}",
                admission.program,
                admission.subcommand.as_deref().unwrap_or("-")
            );
            println!("rendered: {}", admission.rendered());
            Ok(0)
        }
        Err(error) if error.kind() == ErrorKind::Refused => {
            eprintln!("REFUSED: {error}");
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

fn run_command(config: &Config, agent: &str, command: &str, args: &Args) -> Result<i32> {
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
        if args.json {
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
    if args.json {
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

fn command_doctor(config: &Config, agent: &str) -> Result<i32> {
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
            let fields = doctor_error_fields(None);
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
    admitted_fields.insert("doctor".to_owned(), json!(true));
    admitted_fields.insert("rendered".to_owned(), json!(admission.rendered()));
    record_audit(
        &log,
        agent,
        &probe,
        "ADMITTED",
        "doctor probe accepted; target resolution and launch have not yet completed",
        admitted_fields,
    );
    println!(
        "herdr-run doctor  (agent={agent}, config={})\n",
        config.source_path.as_deref().unwrap_or("defaults")
    );
    let mut inside_command = Command::new("/bin/bash");
    inside_command.args(["-lc", &probe]);
    let inside = match bounded_output(&mut inside_command, std::time::Duration::from_secs(120)) {
        Ok(output) => output,
        Err(error) => {
            let error =
                HerdrRunError::unavailable(format!("cannot run the in-jail doctor probe: {error}"));
            record_audit(
                &log,
                agent,
                &probe,
                error_verdict(error.kind()),
                error.message(),
                doctor_error_fields(None),
            );
            return Err(error);
        }
    };
    let inside_code = inside.status.code().unwrap_or(1);
    let inside_ok = inside_code == 0;
    println!("[in-jail ] {probe}");
    println!(
        "           rc={inside_code} {}",
        if inside_ok { "SUCCEEDED" } else { "failed" }
    );
    if !inside_ok {
        let stderr = String::from_utf8_lossy(&inside.stderr);
        println!("           {}", stderr.lines().last().unwrap_or(""));
    }

    // Every failure after the in-jail half of the diagnosis is a doctor result, not a raw wrapper
    // exit. Keep construction, target/cwd resolution, and execution in one result so they all
    // produce the same `[via pane] FAILED` line, final verdict, exit 1, and audit metadata.
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
    let outside_code = match outside {
        Ok((target, result)) => {
            let (meta, meta_error) = write_meta_best_effort(&result, &admission, config, agent);
            let mut fields = doctor_error_fields(Some(inside_code));
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
                &format!("doctor pane probe exited {}", result.exit_code),
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
            let fields = doctor_error_fields(Some(inside_code));
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
    if outside_code == 0 && !inside_ok {
        println!("VERDICT: working as intended — blocked in-jail, succeeds through the pane.");
        Ok(0)
    } else if outside_code == 0 {
        println!("VERDICT: the pane works, but so does the in-jail path. herdr-run is not buying you anything here; prefer running the command directly.");
        Ok(0)
    } else {
        println!("VERDICT: the pane path is NOT working. Most likely the Herdr server was started from inside a sandboxed process, so its panes inherit the confinement. Stop it ('herdr server stop') and let herdr-run restart it via systemd-run, or start it from an unconfined shell.");
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

fn doctor_error_fields(inside_exit_code: Option<i32>) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("doctor".to_owned(), json!(true));
    if let Some(code) = inside_exit_code {
        fields.insert("inside_exit_code".to_owned(), json!(code));
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
    "herdr-run [-h] [--version] [--userguide] [--config PATH] [--agent NAME] [--cwd PATH] [--timeout SECONDS] [--wait-ready SECONDS] [--no-cache] [--json] [--dry-run] [ARG ...]"
}

fn print_help() {
    println!("usage: {}", usage_line());
    println!("\nRun an allowlisted command in a Herdr pane outside the agent sandbox.");
    println!("\npositional arguments:\n  ARG                   '<agent> <command>', '<command>', or check/doctor/config/target/userguide");
    println!("\noptions:\n  -h, --help            show this help message and exit\n  --version             show version and exit\n  --userguide           print the full embedded user guide\n  --config PATH         explicit configuration file\n  --agent NAME          override the invoking agent\n  --cwd PATH            command working directory\n  --timeout SECONDS     command completion timeout\n  --wait-ready SECONDS  pane readiness timeout\n  --no-cache            re-resolve session labels\n  --json                emit a JSON result\n  --dry-run             admit and render without execution");
    println!("\nFull documentation: herdr-run userguide");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_options_between_positionals() {
        let ParseResult::Args(args) = parse([
            OsString::from("agent"),
            OsString::from("--cwd"),
            OsString::from("/tmp"),
            OsString::from("git status"),
            OsString::from("--dry-run"),
        ])
        .expect("parse") else {
            panic!("unexpected early exit")
        };
        assert_eq!(args.positional, ["agent", "git status"]);
        assert_eq!(args.cwd.as_deref(), Some("/tmp"));
        assert!(args.dry_run);
    }

    #[test]
    fn invalid_cli_timeouts_are_rejected() {
        for value in ["nan", "inf", "-1", "1e300"] {
            assert!(parse([OsString::from("--timeout"), OsString::from(value)]).is_err());
        }
    }

    #[test]
    fn option_values_cannot_be_stolen_by_help_or_version() {
        assert!(parse([OsString::from("--agent"), OsString::from("--help")]).is_err());
        assert!(parse([OsString::from("--cwd"), OsString::from("--version")]).is_err());

        let ParseResult::Args(args) =
            parse([OsString::from("--"), OsString::from("--help")]).unwrap()
        else {
            panic!("help after -- must be command data")
        };
        assert_eq!(args.positional, ["--help"]);
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
    fn doctor_failures_have_one_verdict_and_metadata_shape() {
        for kind in [
            ErrorKind::Config,
            ErrorKind::Unavailable,
            ErrorKind::Busy,
            ErrorKind::Timeout,
            ErrorKind::Other,
        ] {
            assert!(!error_verdict(kind).is_empty());
            let fields = doctor_error_fields(Some(17));
            assert_eq!(fields.get("doctor"), Some(&json!(true)));
            assert_eq!(fields.get("inside_exit_code"), Some(&json!(17)));
        }

        let refusal = doctor_error_fields(None);
        assert_eq!(refusal.get("doctor"), Some(&json!(true)));
        assert!(!refusal.contains_key("inside_exit_code"));
        assert_eq!(error_verdict(ErrorKind::Refused), "REFUSED");
    }
}
