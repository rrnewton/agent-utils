//! Command-line interface for durable interactive-agent messaging through Herdr.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;

use crate::agent::{self, AgentError, DrainOptions, QueueOutcome, Target};
use crate::client::HerdrClient;
use crate::error::HerdrRunError;

const COMMANDS: [&str; 5] = ["send", "drain", "status", "read", "userguide"];
const MAX_WAIT_SECONDS: f64 = 31_536_000.0;
const MAX_COUNT: u64 = 1_000_000;

#[derive(Clone, Debug, PartialEq)]
struct Args {
    positional: Vec<String>,
    file: Option<PathBuf>,
    pane: Option<String>,
    session_agent: Option<String>,
    session: Option<String>,
    expected_agent: Option<String>,
    expected_workspace: Option<String>,
    expected_cwd: Option<PathBuf>,
    queue: PathBuf,
    ready_timeout: f64,
    working_timeout: f64,
    max_attempts: u64,
    lines: usize,
    herdr_bin: PathBuf,
    userguide: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            positional: Vec::new(),
            file: None,
            pane: None,
            session_agent: None,
            session: None,
            expected_agent: None,
            expected_workspace: None,
            expected_cwd: None,
            queue: PathBuf::from(".herdr-agent"),
            ready_timeout: 900.0,
            working_timeout: 30.0,
            max_attempts: 3,
            lines: 500,
            herdr_bin: PathBuf::from("herdr"),
            userguide: false,
        }
    }
}

enum ParseResult {
    Args(Box<Args>),
    Exit(i32),
}

/// Run the `herdr-agent` command over an argument iterator.
pub fn main<I>(arguments: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let parsed = match parse(arguments) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("usage: {}", usage_line());
            eprintln!("herdr-agent: error: {message}");
            return 2;
        }
    };
    let args = match parsed {
        ParseResult::Args(args) => *args,
        ParseResult::Exit(code) => return code,
    };
    if let Err(message) = validate_positionals(&args) {
        eprintln!("usage: {}", usage_line());
        eprintln!("herdr-agent: error: {message}");
        return 2;
    }
    if args.userguide
        || args
            .positional
            .first()
            .is_some_and(|value| value == "userguide")
    {
        print!("{}", crate::AGENT_USER_GUIDE);
        return 0;
    }
    if args.positional.is_empty() {
        print_help();
        return 0;
    }
    if let Err(message) = validate(&args) {
        eprintln!("usage: {}", usage_line());
        eprintln!("herdr-agent: error: {message}");
        return 2;
    }
    match run(args) {
        Ok(code) => code,
        Err(CliError::Agent(error)) => emit_agent_error(&error),
        Err(CliError::Herdr(error)) => {
            eprintln!("herdr-agent: {error}");
            error.exit_code()
        }
        Err(CliError::Usage(message)) => {
            eprintln!("herdr-agent: {message}");
            2
        }
        Err(CliError::Output(error)) => {
            eprintln!("herdr-agent: cannot write output: {error}");
            1
        }
    }
}

#[derive(Debug)]
enum CliError {
    Agent(AgentError),
    Herdr(HerdrRunError),
    Usage(String),
    Output(io::Error),
}

impl From<AgentError> for CliError {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
    }
}

impl From<HerdrRunError> for CliError {
    fn from(error: HerdrRunError) -> Self {
        Self::Herdr(error)
    }
}

fn run(args: Args) -> Result<i32, CliError> {
    let command = args
        .positional
        .first()
        .expect("validated command is present")
        .as_str();
    let message = if command == "send" {
        Some(message(&args)?)
    } else {
        None
    };
    let client = HerdrClient::with_executable("direct", &args.herdr_bin)?;
    let target = Target {
        pane_id: args.pane,
        session_agent: args.session_agent,
        session_value: args.session,
        expected_agent: args.expected_agent,
        expected_workspace: args.expected_workspace,
        expected_cwd: args.expected_cwd,
    };
    let queue = absolute_lexical(&args.queue).map_err(|error| {
        CliError::Usage(format!(
            "cannot resolve queue path {}: {error}",
            args.queue.display()
        ))
    })?;
    let options = DrainOptions {
        ready_timeout: Duration::from_secs_f64(args.ready_timeout),
        working_timeout: Duration::from_secs_f64(args.working_timeout),
        max_attempts: args.max_attempts,
    };
    match command {
        "send" => {
            let result = agent::send(
                &client,
                &target,
                &queue,
                message.as_deref().expect("send message was loaded"),
                options,
            )?;
            write_json(&result)?;
            Ok(0)
        }
        "drain" => {
            let result = agent::drain(&client, &target, &queue, options)?;
            let code = if result.blocked.is_some() {
                75
            } else if result.quarantined.is_empty() {
                0
            } else {
                76
            };
            write_json(&result)?;
            Ok(code)
        }
        "status" => {
            write_json(&agent::status(&client, &target, &queue)?)?;
            Ok(0)
        }
        "read" => {
            print!("{}", agent::read(&client, &target, args.lines)?);
            Ok(0)
        }
        _ => unreachable!("parser restricted commands"),
    }
}

fn emit_agent_error(error: &AgentError) -> i32 {
    if let Some(message) = error.undelivered() {
        let document = json!({
            "outcome": error.outcome().map(QueueOutcome::as_str),
            "message_id": message.message_id,
            "artifact": message.artifact,
            "error": error.to_string(),
            "safe_to_retry": error.safe_to_retry(),
        });
        if let Err(output_error) = write_json(&document) {
            eprintln!("herdr-agent: cannot write output: {output_error:?}");
            return 1;
        }
    } else {
        eprintln!("herdr-agent: {error}");
    }
    error.exit_code()
}

fn write_json(value: &impl Serialize) -> Result<(), CliError> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let canonical =
        serde_json::to_value(value).map_err(|error| CliError::Output(io::Error::other(error)))?;
    serde_json::to_writer_pretty(&mut output, &canonical)
        .map_err(|error| CliError::Output(io::Error::other(error)))?;
    output.write_all(b"\n").map_err(CliError::Output)
}

fn message(args: &Args) -> Result<String, CliError> {
    let text = args.positional.get(1);
    match (&args.file, text) {
        (Some(_), Some(_)) => Err(CliError::Usage(
            "pass message text or --file, not both".to_owned(),
        )),
        (Some(path), None) => fs::read_to_string(path).map_err(|error| {
            CliError::Usage(format!(
                "cannot read message artifact {}: {error}",
                path.display()
            ))
        }),
        (None, Some(text)) => Ok(text.clone()),
        (None, None) => Err(CliError::Usage(
            "send needs message text or --file".to_owned(),
        )),
    }
}

fn validate(args: &Args) -> Result<(), String> {
    validate_positionals(args)?;
    let command = args
        .positional
        .first()
        .ok_or_else(|| "a command is required".to_owned())?;
    debug_assert!(COMMANDS.contains(&command.as_str()));
    if !args.ready_timeout.is_finite()
        || !args.working_timeout.is_finite()
        || args.ready_timeout < 0.0
        || args.working_timeout <= 0.0
        || args.ready_timeout > MAX_WAIT_SECONDS
        || args.working_timeout > MAX_WAIT_SECONDS
        || args.max_attempts == 0
        || args.lines == 0
    {
        return Err(format!(
            "timeouts must be finite and positive (ready-timeout may be zero and must not exceed {MAX_WAIT_SECONDS:.0}s); max-attempts and lines must be positive"
        ));
    }
    Ok(())
}

fn validate_positionals(args: &Args) -> Result<(), String> {
    if args.positional.len() > 2 {
        return Err(format!(
            "unrecognized arguments: {}",
            args.positional[2..].join(" ")
        ));
    }
    if let Some(command) = args.positional.first() {
        if !COMMANDS.contains(&command.as_str()) {
            return Err(format!("invalid command: {command:?}"));
        }
    }
    Ok(())
}

fn parse<I>(arguments: I) -> Result<ParseResult, String>
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
        .collect::<Result<Vec<_>, _>>()?;
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
                    println!("herdr-agent {}", env!("CARGO_PKG_VERSION"));
                    return Ok(ParseResult::Exit(0));
                }
                "--userguide" => args.userguide = true,
                option if value_option(option).is_some() => {
                    let value = raw
                        .get(index + 1)
                        .ok_or_else(|| format!("argument {option}: expected one value"))?
                        .clone();
                    if value.starts_with('-') && value != "-" {
                        return Err(format!("argument {option}: expected one value"));
                    }
                    assign(&mut args, option, value)?;
                    index += 1;
                }
                option if option.starts_with("--") && option.contains('=') => {
                    let (name, value) = option.split_once('=').expect("guarded equals");
                    if value_option(name).is_none() {
                        return Err(format!("unrecognized arguments: {option}"));
                    }
                    assign(&mut args, name, value.to_owned())?;
                }
                option if option.starts_with('-') => {
                    return Err(format!("unrecognized arguments: {option}"));
                }
                _ => args.positional.push(token.clone()),
            }
        } else {
            args.positional.push(token.clone());
        }
        index += 1;
    }
    Ok(ParseResult::Args(Box::new(args)))
}

fn value_option(option: &str) -> Option<()> {
    matches!(
        option,
        "--file"
            | "--pane"
            | "--session-agent"
            | "--session"
            | "--agent"
            | "--workspace"
            | "--cwd"
            | "--queue"
            | "--ready-timeout"
            | "--working-timeout"
            | "--max-attempts"
            | "--lines"
            | "--herdr-bin"
    )
    .then_some(())
}

fn assign(args: &mut Args, option: &str, value: String) -> Result<(), String> {
    match option {
        "--file" => args.file = Some(PathBuf::from(value)),
        "--pane" => args.pane = Some(value),
        "--session-agent" => args.session_agent = Some(value),
        "--session" => args.session = Some(value),
        "--agent" => args.expected_agent = Some(value),
        "--workspace" => args.expected_workspace = Some(value),
        "--cwd" => args.expected_cwd = Some(PathBuf::from(value)),
        "--queue" => args.queue = PathBuf::from(value),
        "--ready-timeout" => args.ready_timeout = parse_float(option, &value)?,
        "--working-timeout" => args.working_timeout = parse_float(option, &value)?,
        "--max-attempts" => {
            args.max_attempts = parse_count(option, &value)?;
        }
        "--lines" => {
            args.lines = usize::try_from(parse_count(option, &value)?)
                .expect("MAX_COUNT fits every supported usize");
        }
        "--herdr-bin" => args.herdr_bin = PathBuf::from(value),
        _ => return Err(format!("unsupported option {option}")),
    }
    Ok(())
}

fn parse_float(option: &str, value: &str) -> Result<f64, String> {
    if !has_ascii_float_grammar(value) {
        return Err(format!(
            "argument {option}: invalid numeric value: {value:?}"
        ));
    }
    value
        .parse::<f64>()
        .map_err(|_| format!("argument {option}: invalid numeric value: {value:?}"))
}

fn has_ascii_float_grammar(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = usize::from(
        bytes
            .first()
            .is_some_and(|byte| matches!(byte, b'+' | b'-')),
    );
    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let mut digits = index - integer_start;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fractional_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        digits += index - fractional_start;
    }
    if digits == 0 {
        return false;
    }
    if bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        index += 1;
        if bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }
    index == bytes.len()
}

fn parse_count(option: &str, value: &str) -> Result<u64, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "argument {option}: invalid integer value: {value:?}"
        ));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("argument {option}: invalid integer value: {value:?}"))?;
    if parsed > MAX_COUNT {
        return Err(format!(
            "argument {option}: value must not exceed {MAX_COUNT}"
        ));
    }
    Ok(parsed)
}

fn absolute_lexical(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn usage_line() -> &'static str {
    "herdr-agent [-h] [--version] [--userguide] [OPTIONS] [{send,drain,status,read,userguide}] [text]"
}

fn print_help() {
    println!(
        "usage: {}\n\nQueue, submit, inspect, and read interactive agents hosted in Herdr panes.\n\npositional arguments:\n  {{send,drain,status,read,userguide}}\n  text\n\noptions:\n  -h, --help\n  --version\n  --userguide\n  --file FILE\n  --pane PANE\n  --session-agent SESSION_AGENT\n  --session SESSION\n  --agent AGENT\n  --workspace WORKSPACE\n  --cwd CWD\n  --queue QUEUE\n  --ready-timeout READY_TIMEOUT\n  --working-timeout WORKING_TIMEOUT\n  --max-attempts MAX_ATTEMPTS\n  --lines LINES\n  --herdr-bin HERDR_BIN",
        usage_line()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parser_accepts_intermixed_options_and_rejects_bad_domains() {
        let ParseResult::Args(args) = parse(strings(&[
            "send",
            "hello",
            "--pane",
            "p1",
            "--ready-timeout=0",
        ]))
        .unwrap() else {
            panic!("unexpected early exit");
        };
        assert_eq!(args.positional, ["send", "hello"]);
        assert_eq!(args.pane.as_deref(), Some("p1"));
        assert_eq!(args.ready_timeout, 0.0);
        assert!(validate(&args).is_ok());

        let mut invalid = args;
        invalid.working_timeout = f64::NAN;
        assert!(validate(&invalid).unwrap_err().contains("finite"));
    }

    #[test]
    fn parser_refuses_stolen_options_and_non_ascii_or_unbounded_numbers() {
        for values in [
            &["status", "--pane", "--help"][..],
            &["status", "--pane", "--version"][..],
            &["status", "--pane", "--queue", "state"][..],
            &["status", "--lines", "1_0"][..],
            &["status", "--lines", "١٢"][..],
            &["status", "--lines", "1000001"][..],
            &["status", "--ready-timeout", "1_0"][..],
            &["status", "--ready-timeout", "١.0"][..],
        ] {
            assert!(parse(strings(values)).is_err(), "accepted {values:?}");
        }
        let ParseResult::Args(args) =
            parse(strings(&["status", "--ready-timeout=.5", "--lines=00017"]))
                .expect("valid ASCII numeric forms")
        else {
            panic!("unexpected early exit");
        };
        assert_eq!(args.ready_timeout, 0.5);
        assert_eq!(args.lines, 17);
    }

    #[test]
    fn message_source_is_exactly_one_of_text_or_file() {
        let mut args = Args {
            positional: vec!["send".to_owned()],
            ..Args::default()
        };
        assert!(matches!(message(&args), Err(CliError::Usage(_))));
        args.positional.push("text".to_owned());
        args.file = Some(PathBuf::from("prompt.md"));
        assert!(matches!(message(&args), Err(CliError::Usage(_))));
    }
}
