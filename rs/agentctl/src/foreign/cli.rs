//! Command dispatcher for persistent foreign-harness workers.
//!
//! Every command runs on the Rust lifecycle; `mcp` (slice B3-8)
//! serves the stdio JSON-RPC tool surface from [`super::mcp`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::daemon::run_runner_default;
use super::help_texts::*;
use super::lifecycle::*;
use super::ownership::ForeignOwnershipError;
use super::registry::*;

const COMMANDS: [&str; 12] = [
    "up", "send", "read", "status", "down", "inbox", "reset", "migrate", "backend", "turn",
    "keeper", "mcp",
];

fn keeper_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{KEEPER_HELP}");
        return 0;
    }
    let parsed = match parse(
        args,
        &["--interval"],
        &["--force"],
        KEEPER_USAGE,
        "herdr-subagents keeper",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.len() != 1 {
        return argparse_error(
            KEEPER_USAGE,
            "herdr-subagents keeper",
            "the following arguments are required: action",
        );
    }
    let action = parsed.positionals[0].as_str();
    if !matches!(action, "ensure" | "retire" | "status") {
        return argparse_error(
            KEEPER_USAGE,
            "herdr-subagents keeper",
            &format!(
                "argument action: invalid choice: '{action}' (choose from ensure, retire, status)"
            ),
        );
    }
    let interval = match parsed.get("--interval") {
        None => super::lifecycle::KEEPER_DEFAULT_INTERVAL_SECONDS,
        Some(raw) => match raw.parse::<i64>() {
            Ok(value) => value,
            Err(_) => {
                return argparse_error(
                    KEEPER_USAGE,
                    "herdr-subagents keeper",
                    &format!("argument --interval: invalid int value: '{raw}'"),
                )
            }
        },
    };
    let base = foreign_base();
    let outcome = match action {
        "ensure" => keeper_ensure_at(&base, interval),
        "retire" => keeper_retire_at(&base, parsed.has("--force")),
        _ => {
            println!("{}", keeper_status_line_at(&base));
            return 0;
        }
    };
    for line in &outcome.lines {
        println!("{line}");
    }
    outcome.code
}

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

fn short_session(sid: Option<&str>) -> String {
    match sid {
        None => "-".to_string(),
        Some(sid) => sid.chars().take(8).collect(),
    }
}

fn up_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{UP_HELP}");
        return 0;
    }
    let mut args: Vec<String> = args.to_vec();
    let mut harness_args: Vec<String> = Vec::new();
    if let Some(split) = args.iter().position(|arg| arg == "--") {
        harness_args = args.drain(split + 1..).collect();
        args.truncate(split);
    }
    let parsed = match parse(
        &args,
        &[
            "--cwd",
            "--model",
            "--harness",
            "--backend",
            "--mode",
            "--brief",
            "--brief-file",
        ],
        &[],
        UP_USAGE,
        "herdr-subagents up",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.len() != 1 {
        return argparse_error(
            UP_USAGE,
            "herdr-subagents up",
            "the following arguments are required: name",
        );
    }
    let Some(cwd) = parsed.get("--cwd") else {
        return argparse_error(
            UP_USAGE,
            "herdr-subagents up",
            "the following arguments are required: --cwd",
        );
    };
    for (name, choices) in [
        ("--harness", &["codex", "agy", "muse"][..]),
        ("--backend", &["tmux", "herdr"][..]),
        ("--mode", &["headless", "tui"][..]),
    ] {
        if let Some(value) = parsed.get(name) {
            if !choices.contains(&value) {
                return argparse_error(
                    UP_USAGE,
                    "herdr-subagents up",
                    &format!(
                        "argument {name}: invalid choice: '{value}' (choose from {})",
                        choices
                            .iter()
                            .map(|choice| format!("'{choice}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }
    }
    let brief = if let Some(file) = parsed.get("--brief-file") {
        match std::fs::read_to_string(file) {
            Ok(text) => Some(text),
            Err(_) => return die(&format!("--brief-file not found: {file}")),
        }
    } else {
        parsed.get("--brief").map(str::to_owned)
    };
    match super::lifecycle::bring_up_agent_at(
        &foreign_base(),
        Path::new("/proc"),
        &parsed.positionals[0],
        cwd,
        brief.as_deref(),
        parsed.get("--model"),
        parsed.get("--harness").unwrap_or("codex"),
        parsed.get("--backend"),
        parsed.get("--mode"),
        &harness_args,
    ) {
        Ok(result) => {
            for note in &result.gc_notes {
                println!("[gc] {note}");
            }
            let agent = &result.agent;
            println!(
                "agent '{}' up ({} / {} / {}, model {}, session {}, target {})",
                agent.name,
                agent.backend,
                agent.mode,
                agent.harness,
                agent.model.as_deref().unwrap_or("<default>"),
                short_session(agent.session_id.as_deref()),
                agent.tmux_target
            );
            println!("  transcript: {}", agent.transcript);
            0
        }
        Err(error) => operation_die(&error),
    }
}

fn migrate_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{MIGRATE_HELP}");
        return 0;
    }
    let parsed = match parse(
        args,
        &["--to", "--mode"],
        &[],
        MIGRATE_USAGE,
        "herdr-subagents migrate",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.len() != 1 {
        return argparse_error(
            MIGRATE_USAGE,
            "herdr-subagents migrate",
            "the following arguments are required: name",
        );
    }
    for (name, choices) in [
        ("--to", &["tmux", "herdr"][..]),
        ("--mode", &["headless", "tui"][..]),
    ] {
        if let Some(value) = parsed.get(name) {
            if !choices.contains(&value) {
                return argparse_error(
                    MIGRATE_USAGE,
                    "herdr-subagents migrate",
                    &format!(
                        "argument {name}: invalid choice: '{value}' (choose from {})",
                        choices
                            .iter()
                            .map(|choice| format!("'{choice}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }
    }
    match super::lifecycle::migrate_agent_at(
        &foreign_base(),
        Path::new("/proc"),
        &parsed.positionals[0],
        parsed.get("--to").unwrap_or("herdr"),
        parsed.get("--mode"),
    ) {
        Ok(result) => {
            println!(
                "agent '{}' migrated {}/{} -> {}/{}; presentation {}; session {}",
                result.name,
                result.from_backend,
                result.from_mode,
                result.to_backend,
                result.to_mode,
                result.tmux_target,
                if result.session_id.is_some() {
                    "preserved"
                } else {
                    "not yet created"
                }
            );
            println!("  transcript: {}", result.transcript);
            if result.to_backend == "herdr" {
                println!(
                    "WARNING: restart any pre-Herdr MCP server before using its subagent tools on this worker; a temporary tmux guard prevents its old garbage collector from archiving the Herdr runner."
                );
            }
            0
        }
        Err(error) => operation_die(&error),
    }
}

fn status_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{STATUS_HELP}");
        return 0;
    }
    let parsed = match parse(args, &[], &[], STATUS_USAGE, "herdr-subagents status") {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.len() > 1 {
        return argparse_error(
            STATUS_USAGE,
            "herdr-subagents status",
            &format!(
                "unrecognized arguments: {}",
                parsed.positionals[1..].join(" ")
            ),
        );
    }
    let base = foreign_base();
    let status = match super::lifecycle::status_snapshot_at(
        &base,
        Path::new("/proc"),
        parsed.positionals.first().map(String::as_str),
        true,
    ) {
        Ok(status) => status,
        Err(error) => return operation_die(&error),
    };
    for note in &status.gc_notes {
        println!("[gc] {note}");
    }
    if status.agents.is_empty() {
        println!("no live subagents");
        return 0;
    }
    let header = format!(
        "{:<16} {:<7} {:<9} {:<8} {:<14} {:<4} {:<16} {:<5} {:<10} {:<20} CWD",
        "NAME",
        "BACKEND",
        "MODE",
        "HARNESS",
        "MODEL",
        "WIN",
        "STATUS",
        "PEND",
        "SESSION",
        "LAST_TURN"
    );
    println!("{header}");
    println!("{}", "-".repeat(header.len()));
    for rec in &status.agents {
        let win = if rec.window_alive {
            "yes"
        } else if rec.presentation_degraded {
            "DEG"
        } else {
            "NO"
        };
        println!(
            "{:<16} {:<7} {:<9} {:<8} {:<14} {:<4} {:<16} {:<5} {:<10} {:<20} {}",
            rec.name,
            rec.backend,
            rec.mode,
            rec.harness,
            rec.model.as_deref().unwrap_or("<default>"),
            win,
            rec.status,
            rec.pending,
            short_session(rec.session_id.as_deref()),
            rec.last_turn_at.as_deref().unwrap_or("-"),
            rec.cwd
        );
    }
    println!();
    for rec in &status.agents {
        if rec.mode == "tui" {
            println!("  {}: Herdr scrollback {}", rec.name, rec.transcript);
        } else {
            println!("  {}: transcript {}", rec.name, rec.transcript);
        }
        if rec.presentation_degraded {
            println!(
                "  {}: runner alive but {} presentation missing; use MCP subagent_recreate_window to restore presentation",
                rec.name, rec.backend
            );
        }
    }
    0
}

fn down_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{DOWN_HELP}");
        return 0;
    }
    let parsed = match parse(
        args,
        &["--grace"],
        &["--all-dead", "--force"],
        DOWN_USAGE,
        "herdr-subagents down",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.len() > 1 {
        return argparse_error(
            DOWN_USAGE,
            "herdr-subagents down",
            &format!(
                "unrecognized arguments: {}",
                parsed.positionals[1..].join(" ")
            ),
        );
    }
    let grace_seconds: f64 = match parsed.get("--grace") {
        None => 10.0,
        Some(raw) => match raw.parse() {
            Ok(value) => value,
            Err(_) => {
                return argparse_error(
                    DOWN_USAGE,
                    "herdr-subagents down",
                    &format!("argument --grace: invalid float value: '{raw}'"),
                )
            }
        },
    };
    let grace = std::time::Duration::try_from_secs_f64(grace_seconds.max(0.0)).unwrap_or_default();
    let base = foreign_base();
    if parsed.has("--all-dead") {
        if parsed.has("--force") {
            return die("--force requires an agent NAME; --all-dead remains conservative");
        }
        return match super::lifecycle::gc_at(&base, Path::new("/proc")) {
            Ok(notes) => {
                if notes.is_empty() {
                    println!("no dead agents to reap");
                } else {
                    for note in notes {
                        println!("[gc] {note}");
                    }
                }
                0
            }
            Err(error) => operation_die(&error),
        };
    }
    let Some(name) = parsed.positionals.first() else {
        return die("give an agent NAME, or use --all-dead");
    };
    if let Err(error) = super::registry::require_valid_name(name) {
        return operation_die(&error);
    }
    match super::lifecycle::bring_down_agent_at(
        &base,
        Path::new("/proc"),
        name,
        grace,
        true,
        parsed.has("--force"),
    ) {
        Ok(res) => {
            if !res.was_registered {
                println!(
                    "agent '{}' was not in the registry; killed_window={}; archived {}",
                    res.name,
                    res.killed_window,
                    res.archived_to.as_deref().unwrap_or("None")
                );
            } else if res.forced {
                println!(
                    "WARNING: forced retirement of '{}'; {} was not probed or closed. State archived to {}; registry entry removed.",
                    res.name,
                    res.unverified_presentation.as_deref().unwrap_or_default(),
                    res.archived_to.as_deref().unwrap_or("None")
                );
            } else {
                println!(
                    "agent '{}' down; window killed; state archived to {}",
                    res.name,
                    res.archived_to.as_deref().unwrap_or("None")
                );
            }
            0
        }
        Err(error) => operation_die(&error),
    }
}

fn wait_for_sentinel(
    transcript: &Path,
    seq: i64,
    timeout: std::time::Duration,
    stream: bool,
    start_offset: u64,
) -> Option<String> {
    use std::io::Seek as _;
    let needle = format!("===TURN-DONE {seq} rc=");
    let deadline = std::time::Instant::now() + timeout;
    let mut offset = start_offset;
    let mut pending = String::new();
    while std::time::Instant::now() < deadline {
        if transcript.exists() {
            if let Ok(mut file) = std::fs::File::open(transcript) {
                if file.seek(std::io::SeekFrom::Start(offset)).is_ok() {
                    let mut chunk = String::new();
                    if std::io::Read::read_to_string(&mut file, &mut chunk).unwrap_or(0) > 0 {
                        offset += chunk.len() as u64;
                        if stream {
                            print!("{chunk}");
                            let _ = std::io::Write::flush(&mut std::io::stdout());
                        }
                        pending.push_str(&chunk);
                        if let Some(index) = pending.find(&needle) {
                            let rest = &pending[index + needle.len()..];
                            let rc: String = rest
                                .chars()
                                .take_while(|ch| !ch.is_whitespace() && *ch != '=')
                                .collect();
                            if !rc.is_empty() {
                                return Some(rc);
                            }
                        }
                        if pending.len() > 65536 {
                            pending = tail_chars(&pending, 4096);
                        }
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    None
}

fn tail_chars(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }
    chars[chars.len() - limit..].iter().collect()
}

fn inbox_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{INBOX_HELP}");
        return 0;
    }
    let parsed = match parse(
        args,
        &[],
        &["--inspect", "--drain", "--clear-composer"],
        INBOX_USAGE,
        "herdr-subagents inbox",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.has("--inspect") && parsed.has("--drain") {
        return argparse_error(
            INBOX_USAGE,
            "herdr-subagents inbox",
            "argument --drain: not allowed with argument --inspect",
        );
    }
    if parsed.has("--clear-composer") && !parsed.has("--drain") {
        return argparse_error(
            INBOX_USAGE,
            "herdr-subagents inbox",
            "--clear-composer requires --drain",
        );
    }
    if parsed.positionals.len() != 1 {
        return argparse_error(
            INBOX_USAGE,
            "herdr-subagents inbox",
            "the following arguments are required: name",
        );
    }
    if parsed.has("--drain") {
        return match super::lifecycle::drain_tui_inbox_at(
            &foreign_base(),
            Path::new("/proc"),
            &parsed.positionals[0],
            parsed.has("--clear-composer"),
        ) {
            Ok(result) => {
                let show = |seqs: &[i64]| {
                    if seqs.is_empty() {
                        "none".to_string()
                    } else {
                        format!(
                            "[{}]",
                            seqs.iter()
                                .map(i64::to_string)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                };
                println!("agent: {}", result.name);
                println!("pending: {}", show(&result.pending));
                println!("quarantined: {}", show(&result.quarantined));
                println!("failed messages: {}", result.failed_dir);
                0
            }
            Err(error) => operation_die(&error),
        };
    }
    match super::lifecycle::tui_inbox_snapshot_at(&foreign_base(), &parsed.positionals[0]) {
        Ok(result) => {
            let show = |seqs: &[i64]| {
                if seqs.is_empty() {
                    "none".to_string()
                } else {
                    format!(
                        "[{}]",
                        seqs.iter()
                            .map(i64::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            };
            println!("agent: {}", result.name);
            println!("pending: {}", show(&result.pending));
            println!("quarantined: {}", show(&result.quarantined));
            println!("failed messages: {}", result.failed_dir);
            0
        }
        Err(error) => operation_die(&error),
    }
}

fn turn_command(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{TURN_HELP}");
        return 0;
    }
    let parsed = match parse(
        args,
        &[
            "--message-file",
            "--cwd",
            "--model",
            "--effort",
            "--harness",
            "--backend",
            "--mode",
            "--timeout",
            "--tail",
        ],
        &["--fresh", "--quiet"],
        TURN_USAGE,
        "herdr-subagents turn",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    if parsed.positionals.is_empty() || parsed.positionals.len() > 2 {
        return argparse_error(
            TURN_USAGE,
            "herdr-subagents turn",
            "the following arguments are required: name",
        );
    }
    for (name, choices) in [
        ("--harness", &["codex", "agy", "muse"][..]),
        ("--backend", &["tmux", "herdr"][..]),
    ] {
        if let Some(value) = parsed.get(name) {
            if !choices.contains(&value) {
                return argparse_error(
                    TURN_USAGE,
                    "herdr-subagents turn",
                    &format!(
                        "argument {name}: invalid choice: '{value}' (choose from {})",
                        choices
                            .iter()
                            .map(|choice| format!("'{choice}'"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }
    }
    let timeout: f64 = match parsed.get("--timeout") {
        None => 3600.0,
        Some(raw) => match raw.parse() {
            Ok(value) => value,
            Err(_) => {
                return argparse_error(
                    TURN_USAGE,
                    "herdr-subagents turn",
                    &format!("argument --timeout: invalid float value: '{raw}'"),
                )
            }
        },
    };
    let tail: i64 = match parsed.get("--tail") {
        None => 80,
        Some(raw) => match raw.parse() {
            Ok(value) => value,
            Err(_) => {
                return argparse_error(
                    TURN_USAGE,
                    "herdr-subagents turn",
                    &format!("argument --tail: invalid int value: '{raw}'"),
                )
            }
        },
    };
    let name = parsed.positionals[0].clone();
    let message = if let Some(file) = parsed.get("--message-file") {
        match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(_) => {
                eprintln!("error: --message-file not found: {file}");
                return 1;
            }
        }
    } else if let Some(text) = parsed.positionals.get(1) {
        text.clone()
    } else {
        let mut data = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut data);
        if data.trim().is_empty() {
            eprintln!("no message: pass text, --message-file, or stdin");
            return 1;
        }
        data
    };
    let base = foreign_base();
    let proc_root = Path::new("/proc");
    let record_for = |base: &Path, name: &str| {
        super::registry::read_registry_at(base)
            .ok()
            .and_then(|agents| agents.get(name).cloned())
    };
    let pre = record_for(&base, &name);
    let mut start_offset = 0_u64;
    if pre.is_some() {
        if let Ok(path) = super::lifecycle::transcript_path_at(&base, &name) {
            start_offset = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        }
    }
    if parsed.has("--fresh") && record_for(&base, &name).is_some() {
        match super::lifecycle::reset_agent_context_at(&base, &name) {
            Ok(res) => {
                let prior = if res.previous_session_id.is_some() {
                    "cleared"
                } else {
                    "already empty"
                };
                println!("[agent_turn] fresh context for {name} ({prior})");
            }
            Err(error) => {
                println!(
                    "[agent_turn] WARNING: could not reset {name} ({}); fresh-context turn was not submitted",
                    error.message
                );
                return 4;
            }
        }
    }
    if let Some(effort) = parsed.get("--effort") {
        std::env::set_var("SUBAGENT_EFFORT", effort);
    }
    if record_for(&base, &name).is_none() {
        if parsed.get("--cwd").is_none() {
            eprintln!("agent '{name}' does not exist; pass --cwd to create it");
            return 1;
        }
        println!(
            "[agent_turn] FAILED to prepare {name}: creating an agent is not yet in this edition"
        );
        return 4;
    }
    // Heal the view best-effort: a gone window degrades the view, never the turn.
    if let Ok(snapshot) = super::lifecycle::status_snapshot_at(&base, proc_root, Some(&name), true)
    {
        if snapshot.agents.iter().any(|agent| !agent.window_alive) {
            println!("[agent_turn] window for {name} is gone — recreating tab/tailer");
            if let Err(error) = super::lifecycle::recreate_window_at(&base, proc_root, &name) {
                println!(
                    "[agent_turn] WARNING: could not recreate window: {}",
                    error.message
                );
            }
        }
    }
    let sent = match super::lifecycle::send_message_to_agent_at(
        &base,
        proc_root,
        &name,
        &message,
        parsed.get("--model"),
    ) {
        Ok(sent) => sent,
        Err(error) => {
            println!("[agent_turn] FAILED to send to {name}: {}", error.message);
            return 4;
        }
    };
    if !sent.quarantined.is_empty() {
        println!(
            "[agent_turn] WARNING: {} earlier message(s) quarantined",
            sent.quarantined.len()
        );
    }
    let transcript = PathBuf::from(&sent.transcript);
    println!(
        "[agent_turn] {name} seq={} transcript={}",
        sent.seq, sent.transcript
    );
    let rc = wait_for_sentinel(
        &transcript,
        sent.seq,
        std::time::Duration::try_from_secs_f64(timeout.max(0.0)).unwrap_or_default(),
        !parsed.has("--quiet"),
        start_offset,
    );
    let Some(rc) = rc else {
        println!(
            "[agent_turn] TIMEOUT after {timeout}s waiting for TURN-DONE {}",
            sent.seq
        );
        return 3;
    };
    println!("[agent_turn] TURN-DONE seq={} rc={rc}", sent.seq);
    if parsed.has("--quiet") && tail > 0 {
        if let Ok(text) = std::fs::read_to_string(&transcript) {
            let lines: Vec<&str> = text.lines().collect();
            let selected = &lines[lines.len().saturating_sub(tail as usize)..];
            println!("{}", selected.join("\n"));
        }
    }
    if rc == "0" {
        0
    } else {
        2
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
        "status" => status_command(rest),
        "down" => down_command(rest),
        "turn" => turn_command(rest),
        "up" => up_command(rest),
        "migrate" => migrate_command(rest),
        "inbox" => inbox_command(rest),
        "keeper" => keeper_command(rest),
        "mcp" => super::mcp::run(),
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
