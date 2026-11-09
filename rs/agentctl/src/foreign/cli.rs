//! Command dispatcher for persistent foreign-harness workers.
//!
//! Headless commands (`backend`, `send`, `read`, `reset`, and the hidden
//! runner entry) run on the Rust lifecycle today; presentation commands
//! refuse by name until their slice lands, rather than silently doing
//! something else.

use std::collections::HashMap;
use std::path::Path;

use super::daemon::run_runner_default;
use super::help_texts::*;
use super::lifecycle::*;
use super::ownership::ForeignOwnershipError;
use super::registry::*;

const COMMANDS: [&str; 12] = [
    "up", "send", "read", "status", "down", "inbox", "reset", "migrate", "backend", "turn",
    "keeper", "mcp",
];

const NOT_PORTED: [&str; 7] = ["up", "status", "down", "inbox", "migrate", "turn", "keeper"];

fn argparse_error(usage: &str, prog: &str, message: &str) -> i32 {
    eprintln!("{usage}\n{prog}: error: {message}");
    2
}

fn die(message: &str) -> i32 {
    eprintln!("error: {message}");
    1
}

fn operation_die(error: &ForeignOwnershipError) -> i32 {
    die(&error.message)
}

struct Parsed {
    values: HashMap<String, Vec<String>>,
    flags: Vec<String>,
    positionals: Vec<String>,
}

fn parse(
    args: &[String],
    value_options: &[&str],
    flag_options: &[&str],
    usage: &str,
    prog: &str,
) -> Result<Parsed, i32> {
    let mut values: HashMap<String, Vec<String>> = HashMap::new();
    let mut flags = Vec::new();
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (head, explicit) = match arg.split_once('=') {
            Some((head, value)) if head.starts_with("--") => (head, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        if flag_options.contains(&head) {
            if let Some(value) = &explicit {
                return Err(argparse_error(
                    usage,
                    prog,
                    &format!("argument {head}: ignored explicit argument '{value}'"),
                ));
            }
            flags.push(head.to_string());
            index += 1;
            continue;
        }
        let candidates: Vec<&str> = value_options
            .iter()
            .copied()
            .filter(|name| head == *name || (head.len() > 2 && name.starts_with(head)))
            .collect();
        if candidates.len() > 1 && !candidates.contains(&head) {
            return Err(argparse_error(
                usage,
                prog,
                &format!(
                    "ambiguous option: {head} could match {}",
                    candidates.join(", ")
                ),
            ));
        }
        if let Some(name) = candidates
            .iter()
            .find(|name| **name == head)
            .or_else(|| candidates.first())
        {
            let name = (*name).to_string();
            let value = match explicit {
                Some(value) => value,
                None => {
                    index += 1;
                    match args.get(index) {
                        Some(value) => value.clone(),
                        None => {
                            return Err(argparse_error(
                                usage,
                                prog,
                                &format!("argument {name}: expected one argument"),
                            ))
                        }
                    }
                }
            };
            values.entry(name).or_default().push(value);
            index += 1;
            continue;
        }
        if arg.starts_with('-') && arg.len() > 1 && arg != "-" {
            return Err(argparse_error(
                usage,
                prog,
                &format!("unrecognized arguments: {arg}"),
            ));
        }
        positionals.push(arg.clone());
        index += 1;
    }
    Ok(Parsed {
        values,
        flags,
        positionals,
    })
}

impl Parsed {
    fn get(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|items| items.last())
            .map(String::as_str)
    }
    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }
}

fn backend_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{BACKEND_HELP}");
        return 0;
    }
    let parsed = match parse(
        args,
        &["--set"],
        &[],
        BACKEND_USAGE,
        "herdr-subagents backend",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if let Some(extra) = parsed.positionals.first() {
        return argparse_error(
            BACKEND_USAGE,
            "herdr-subagents backend",
            &format!("unrecognized arguments: {extra}"),
        );
    }
    let base = foreign_base();
    let outcome = (|| -> Result<String, ForeignOwnershipError> {
        if let Some(set) = parsed.get("--set") {
            if !matches!(set, "tmux" | "herdr") {
                return Err(ForeignOwnershipError {
                    code: "unsupported_backend",
                    message: format!(
                        "argument --set: invalid choice: '{}' (choose from 'tmux', 'herdr')",
                        set
                    ),
                });
            }
            let backend = set_default_backend_at(&base, set)?;
            Ok(format!(
                "saved default backend '{backend}' in {}",
                backend_config_path(&base).display()
            ))
        } else {
            Ok(format!(
                "selected backend: {}\nconfig: {}",
                selected_backend_at(&base, None)?,
                backend_config_path(&base).display()
            ))
        }
    })();
    match outcome {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(error) => operation_die(&error),
    }
}

fn send_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{SEND_HELP}");
        return 0;
    }
    let parsed = match parse(
        args,
        &["--message-file", "--model"],
        &[],
        SEND_USAGE,
        "herdr-subagents send",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.is_empty() {
        return argparse_error(
            SEND_USAGE,
            "herdr-subagents send",
            "the following arguments are required: name",
        );
    }
    if parsed.positionals.len() > 2 {
        return argparse_error(
            SEND_USAGE,
            "herdr-subagents send",
            &format!(
                "unrecognized arguments: {}",
                parsed.positionals[2..].join(" ")
            ),
        );
    }
    let name = parsed.positionals[0].clone();
    let text = if let Some(file) = parsed.get("--message-file") {
        let path = Path::new(file);
        if !path.is_file() {
            return die(&format!("--message-file not found: {}", path.display()));
        }
        match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => return die(&format!("cannot read {}: {error}", path.display())),
        }
    } else {
        match parsed.positionals.get(1) {
            Some(text) => text.clone(),
            None => return die("no message: pass text positionally or via --message-file"),
        }
    };
    if text.is_empty() {
        return die("no message: pass text positionally or via --message-file");
    }
    let model = parsed.get("--model");
    match send_message_to_agent_at(&foreign_base(), Path::new("/proc"), &name, &text, model) {
        Ok(res) => {
            let override_note = model
                .map(|model| format!(" (model override: {model})"))
                .unwrap_or_default();
            println!(
                "queued message as turn {} for '{}'{}",
                res.seq, res.name, override_note
            );
            println!(
                "  watch: tail -f {}  (sentinel: TURN-DONE {})",
                res.transcript, res.seq
            );
            0
        }
        Err(error) => operation_die(&error),
    }
}

fn read_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{READ_HELP}");
        return 0;
    }
    let parsed = match parse(
        args,
        &["--since-turn", "--tail"],
        &["--last"],
        READ_USAGE,
        "herdr-subagents read",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.len() != 1 {
        return argparse_error(
            READ_USAGE,
            "herdr-subagents read",
            "the following arguments are required: name",
        );
    }
    let parse_i64 = |name: &str| -> Result<Option<i64>, i32> {
        match parsed.get(name) {
            None => Ok(None),
            Some(raw) => raw.parse::<i64>().map(Some).map_err(|_| {
                argparse_error(
                    READ_USAGE,
                    "herdr-subagents read",
                    &format!("argument {name}: invalid int value: '{raw}'"),
                )
            }),
        }
    };
    let since_turn = match parse_i64("--since-turn") {
        Ok(value) => value,
        Err(code) => return code,
    };
    let tail = match parse_i64("--tail") {
        Ok(value) => value,
        Err(code) => return code,
    };
    let selected = [parsed.has("--last"), since_turn.is_some(), tail.is_some()]
        .iter()
        .filter(|flag| **flag)
        .count();
    if selected > 1 {
        return argparse_error(
            READ_USAGE,
            "herdr-subagents read",
            "argument --since-turn: not allowed with argument --last",
        );
    }
    let mode = if parsed.has("--last") {
        "last"
    } else if since_turn.is_some() {
        "since_turn"
    } else if tail.is_some() {
        "tail"
    } else {
        "all"
    };
    match read_agent_output_at(
        &foreign_base(),
        &parsed.positionals[0],
        mode,
        since_turn,
        tail,
    ) {
        Ok(res) => {
            print!("{}", res.text);
            if !res.text.ends_with('\n') {
                println!();
            }
            0
        }
        Err(error) => operation_die(&error),
    }
}

fn reset_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{RESET_HELP}");
        return 0;
    }
    let parsed = match parse(args, &[], &[], RESET_USAGE, "herdr-subagents reset") {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.len() != 1 {
        return argparse_error(
            RESET_USAGE,
            "herdr-subagents reset",
            "the following arguments are required: name",
        );
    }
    match reset_agent_context_at(&foreign_base(), &parsed.positionals[0]) {
        Ok(result) => {
            let previous = if result.previous_session_id.is_some() {
                "present"
            } else {
                "already empty"
            };
            println!(
                "agent '{}' context reset ({previous}); transcript: {}",
                result.name, result.transcript
            );
            0
        }
        Err(error) => operation_die(&error),
    }
}

/// Run the persistent-worker dispatcher with the supplied command arguments.
pub fn main(arguments: &[String]) -> i32 {
    if arguments == ["--version"] {
        println!("herdr-subagents {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if arguments == ["--userguide"] {
        print!("{}", include_str!("../embedded_userguide.md"));
        return 0;
    }
    if arguments.is_empty() || matches!(arguments[0].as_str(), "-h" | "--help") {
        print!("{TOP_HELP}");
        return 0;
    }
    // Internal runner entry (the presentation wrapper's command); not a
    // documented subcommand.
    if arguments[0] == "__runner" {
        let Some(name) = arguments.get(1) else {
            eprintln!("error: usage: herdr-subagents __runner <name>");
            return 1;
        };
        let stage = std::env::var("SUBAGENTS_MIGRATION_STAGE").ok();
        return match run_runner_default(name, stage) {
            Ok(code) => code,
            Err(error) => operation_die(&error),
        };
    }
    let command = arguments[0].as_str();
    if !COMMANDS.contains(&command) {
        let choices = COMMANDS.join(",");
        return argparse_error(
            TOP_USAGE,
            "herdr-subagents",
            &format!("argument command: invalid choice: '{command}' (choose from {choices})"),
        );
    }
    let rest = &arguments[1..];
    match command {
        "backend" => backend_command(rest),
        "send" => send_command(rest),
        "read" => read_command(rest),
        "reset" => reset_command(rest),
        "mcp" => {
            eprintln!("herdr-subagents: the Rust edition has not ported 'mcp' yet");
            2
        }
        other if NOT_PORTED.contains(&other) => {
            eprintln!("herdr-subagents: the Rust edition has not ported '{other}' yet");
            2
        }
        _ => unreachable!("command list checked"),
    }
}

/// The dispatcher entry at a path (used by the binary's main).
pub fn main_entry() -> i32 {
    let arguments: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    main(&arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foreign::lifecycle::{pid_start_time_at, transcript_path_at};
    use crate::foreign::ownership::current_boot_id_at;
    use crate::foreign::registry::{with_registry_lock_at, AgentRecord};

    #[test]
    fn headless_commands_run_against_a_fake_base() {
        let base = std::env::temp_dir().join(format!("agentctl-b3-7c-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::env::set_var("HERDR_SUBAGENTS_HOME", &base);
        std::env::remove_var("SUBAGENTS_BACKEND");
        std::env::remove_var("HERDR_ENV");

        assert_eq!(main(&["--version".to_string()]), 0);
        assert_eq!(main(&["backend".to_string()]), 0);
        assert_eq!(
            main(&[
                "backend".to_string(),
                "--set".to_string(),
                "herdr".to_string()
            ]),
            0
        );
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(base.join("backend.json")).unwrap()).unwrap();
        assert_eq!(config["backend"], "herdr");

        // Seed a live headless worker whose runner is this test process.
        let own = std::process::id() as i64;
        with_registry_lock_at(&base, |agents| {
            agents.insert(
                "worker-1".to_string(),
                AgentRecord {
                    name: "worker-1".to_string(),
                    harness: "codex".to_string(),
                    backend: "tmux".to_string(),
                    status: "idle".to_string(),
                    runner_pid: Some(own),
                    runner_started_at: pid_start_time_at(Path::new("/proc"), own).unwrap(),
                    runner_boot_id: Some(current_boot_id_at(Path::new("/proc")).unwrap()),
                    next_seq: 1,
                    created_at: "2026-11-07T00:00:00Z".to_string(),
                    ..AgentRecord::default()
                },
            );
            Ok(())
        })
        .unwrap();
        crate::foreign::lifecycle::ensure_agent_dirs_at(&base, "worker-1").unwrap();
        std::fs::write(
            transcript_path_at(&base, "worker-1").unwrap(),
            "line one\nline two\n",
        )
        .unwrap();
        std::fs::write(
            crate::foreign::lifecycle::last_message_path_at(&base, "worker-1").unwrap(),
            "the answer",
        )
        .unwrap();

        assert_eq!(
            main(&[
                "send".to_string(),
                "worker-1".to_string(),
                "hello there".to_string()
            ]),
            0
        );
        assert_eq!(
            main(&[
                "read".to_string(),
                "worker-1".to_string(),
                "--last".to_string()
            ]),
            0
        );
        // Reset refuses while the just-queued turn is pending, then succeeds
        // once it is taken out of the inbox (the idle boundary).
        assert_eq!(main(&["reset".to_string(), "worker-1".to_string()]), 1);
        let pending = crate::foreign::lifecycle::next_pending_message_at(&base, "worker-1")
            .unwrap()
            .unwrap();
        std::fs::remove_file(pending).unwrap();
        assert_eq!(main(&["reset".to_string(), "worker-1".to_string()]), 0);
        assert_eq!(main(&["up".to_string(), "worker-1".to_string()]), 2);
        assert_eq!(main(&["bogus".to_string()]), 2);
        std::env::remove_var("HERDR_SUBAGENTS_HOME");
        let _ = std::fs::remove_dir_all(&base);
    }
}
