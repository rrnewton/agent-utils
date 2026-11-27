//! Coordinator-lean launches (#2322 agentctl-coordinator-lean-headless-profile).
//!
//! A lean coordinator answers the owner and steers its lanes. It never runs long or writing
//! commands itself: those go to a helper lane, its "hands". A launch profile with
//! `coordinator_lean` makes agentctl start Claude Code with an agentctl-owned settings file:
//!
//! - a Bash time ceiling that Claude Code enforces itself (`BASH_MAX_TIMEOUT_MS`, with background
//!   Bash off so a call that reaches the ceiling is killed rather than moved to the background);
//! - deny rules that remove the file-writing, web, scheduling and subagent tools;
//! - a PreToolUse hook, `agentctl lean-hook`, that lets through only short read-only commands,
//!   `agentctl send` and `interrupt` to the agent's own lanes, and agent-teams messaging, and
//!   refuses everything else with a reason that names the hands lane.
//!
//! Allow-lists are deliberately not used. Installed plugin hooks can answer `allow` and so
//! bypass them, while a hook's `deny` and a settings `deny` rule hold, also under
//! `--dangerously-skip-permissions` (measured with Claude Code 2.1.319). The hook command ends in
//! `|| exit 2`, so a hook that cannot run blocks the call instead of letting it through.
//!
//! The classifier here is conservative on purpose: it understands a small, safe subset of shell
//! (words, quotes, pipes, `&&`, `||`, `;`, and redirection to `/dev/null`) and refuses anything
//! it does not understand.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};

/// The Bash time ceiling a lean profile gets when it names none, in seconds.
pub(crate) const DEFAULT_BASH_TIMEOUT_SECONDS: u64 = 30;
/// The shortest Bash time ceiling a lean profile may name, in seconds.
pub(crate) const MIN_BASH_TIMEOUT_SECONDS: u64 = 10;
/// The longest Bash time ceiling a lean profile may name, in seconds.
pub(crate) const MAX_BASH_TIMEOUT_SECONDS: u64 = 60;
/// Seconds Claude Code waits for the hook before it counts as failed.
const HOOK_TIMEOUT_SECONDS: u64 = 10;
/// Most extra commands or lanes one profile may list.
const MAX_LIST: usize = 32;
/// Longest extra command, in bytes.
const MAX_EXTRA_BYTES: usize = 256;
/// Largest operator settings file merged into the lean settings, in bytes.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
/// The directory under the registry that holds each lean agent's rendered settings file.
const SETTINGS_DIRECTORY: &str = "lean-settings";

/// Tools a lean coordinator keeps; the hook passes them without a decision.
const PASS_TOOLS: [&str; 6] = [
    "Read",
    "ToolSearch",
    "Skill",
    "CronList",
    "ListAgents",
    "TodoWrite",
];

/// Tools the settings deny outright, so the model never sees them (Claude Code 2.1.319 names).
/// The hook refuses any tool not in [`PASS_TOOLS`] as well, so a tool a later Claude Code adds
/// is refused until it is classified.
const DENIED_TOOLS: [&str; 16] = [
    "Edit",
    "Write",
    "NotebookEdit",
    "Task",
    "Agent",
    "Workflow",
    "Monitor",
    "CronCreate",
    "CronDelete",
    "ScheduleWakeup",
    "WebFetch",
    "WebSearch",
    "EnterWorktree",
    "ExitWorktree",
    "SendMessage",
    "TaskStop",
];

/// Commands the settings deny as well as the hook, as a second line of defence.
const DENIED_BASH: [&str; 12] = [
    "Bash(sudo:*)",
    "Bash(su:*)",
    "Bash(rm:*)",
    "Bash(mv:*)",
    "Bash(dd:*)",
    "Bash(curl:*)",
    "Bash(wget:*)",
    "Bash(ssh:*)",
    "Bash(dnf:*)",
    "Bash(rpm:*)",
    "Bash(pip:*)",
    "Bash(npm:*)",
];

/// One profile's `coordinator_lean` setting.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct LeanPolicy {
    /// Longest a Bash call may run, in seconds.
    pub(crate) bash_timeout_seconds: u64,
    /// Operator-added read-only commands, each a space-separated argv prefix.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) extra_commands: Vec<String>,
    /// Agents `send` and `interrupt` may name besides the agent's own lanes (its hands lane when
    /// no managed_by edge records it).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) lanes: Vec<String>,
}

impl Default for LeanPolicy {
    fn default() -> Self {
        Self {
            bash_timeout_seconds: DEFAULT_BASH_TIMEOUT_SECONDS,
            extra_commands: Vec::new(),
            lanes: Vec::new(),
        }
    }
}

fn valid_agent_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn valid_extra_command(command: &str) -> bool {
    !command.trim().is_empty()
        && command.len() <= MAX_EXTRA_BYTES
        && !command.chars().any(|character| {
            character.is_control() || "'\"\\$`|&;<>(){}*?[]#~!=".contains(character)
        })
}

/// Parse a profile's `coordinator_lean` value: absent, `null` or `false` is off, `true` is the
/// defaults, and an object may set `bash_timeout_seconds`, `extra_commands` and `lanes`.
pub(crate) fn policy_from_value(
    value: Option<&Value>,
    label: &str,
) -> Result<Option<LeanPolicy>, String> {
    let object = match value {
        None | Some(Value::Null | Value::Bool(false)) => return Ok(None),
        Some(Value::Bool(true)) => return Ok(Some(LeanPolicy::default())),
        Some(Value::Object(object)) => object,
        Some(_) => {
            return Err(format!(
                "{label} coordinator_lean must be true, false, or an object"
            ))
        }
    };
    let mut policy = LeanPolicy::default();
    for (key, item) in object {
        match key.as_str() {
            "bash_timeout_seconds" => {
                let seconds = item.as_u64().filter(|seconds| {
                    (MIN_BASH_TIMEOUT_SECONDS..=MAX_BASH_TIMEOUT_SECONDS).contains(seconds)
                });
                policy.bash_timeout_seconds = seconds.ok_or_else(|| {
                    format!(
                        "{label} coordinator_lean.bash_timeout_seconds must be a whole number \
                         from {MIN_BASH_TIMEOUT_SECONDS} to {MAX_BASH_TIMEOUT_SECONDS}"
                    )
                })?;
            }
            "extra_commands" => {
                policy.extra_commands = string_list(item, label, key)?;
                if let Some(bad) = policy
                    .extra_commands
                    .iter()
                    .find(|command| !valid_extra_command(command))
                {
                    return Err(format!(
                        "{label} coordinator_lean.extra_commands entry {bad:?} must be plain words \
                         (no quotes, shell operators, globs or control characters), at most \
                         {MAX_EXTRA_BYTES} bytes"
                    ));
                }
            }
            "lanes" => {
                policy.lanes = string_list(item, label, key)?;
                if let Some(bad) = policy.lanes.iter().find(|name| !valid_agent_name(name)) {
                    return Err(format!(
                        "{label} coordinator_lean.lanes entry {bad:?} must be an agent name \
                         ([a-z][a-z0-9-]{{0,31}})"
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "{label} coordinator_lean has unknown key {key:?}; use bash_timeout_seconds, \
                     extra_commands and/or lanes"
                ))
            }
        }
    }
    Ok(Some(policy))
}

fn string_list(value: &Value, label: &str, key: &str) -> Result<Vec<String>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{label} coordinator_lean.{key} must be an array of strings"))?;
    if items.len() > MAX_LIST {
        return Err(format!(
            "{label} coordinator_lean.{key} lists more than {MAX_LIST} entries"
        ));
    }
    let mut list: Vec<String> = Vec::new();
    for item in items {
        let text = item
            .as_str()
            .ok_or_else(|| format!("{label} coordinator_lean.{key} must be an array of strings"))?;
        if list.iter().any(|existing| existing == text) {
            return Err(format!(
                "{label} coordinator_lean.{key} names {text:?} twice"
            ));
        }
        list.push(text.to_owned());
    }
    Ok(list)
}

/// What a lean launch renders its settings for.
pub(crate) struct LeanLaunch<'a> {
    /// The profile's setting.
    pub(crate) policy: &'a LeanPolicy,
    /// The agentctl executable the hook runs.
    pub(crate) agentctl: &'a Path,
    /// The agent registry the agent belongs to.
    pub(crate) registry: &'a Path,
    /// The lean agent's name.
    pub(crate) agent: &'a str,
}

/// Where the rendered settings of lean agent `agent` live: one file per agent, in a directory of
/// its own under the registry.
pub(crate) fn settings_path(registry: &Path, agent: &str) -> PathBuf {
    registry
        .join(SETTINGS_DIRECTORY)
        .join(format!("{agent}.json"))
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The hook command line the settings run before every tool call. It ends in `|| exit 2`, so a
/// hook that cannot run (a missing binary exits 127, a non-blocking error to Claude Code) blocks
/// the call instead of letting it through.
pub(crate) fn hook_command(launch: &LeanLaunch<'_>) -> String {
    let mut words = vec![
        launch.agentctl.display().to_string(),
        "--registry".to_owned(),
        launch.registry.display().to_string(),
        "lean-hook".to_owned(),
        "--agent".to_owned(),
        launch.agent.to_owned(),
        "--bash-timeout".to_owned(),
        launch.policy.bash_timeout_seconds.to_string(),
    ];
    for lane in &launch.policy.lanes {
        words.push("--lane".to_owned());
        words.push(lane.clone());
    }
    for command in &launch.policy.extra_commands {
        words.push("--extra-command".to_owned());
        words.push(command.clone());
    }
    let quoted: Vec<String> = words.iter().map(String::as_str).map(shell_quote).collect();
    format!("{} || exit 2", quoted.join(" "))
}

/// The settings a lean launch adds: the Bash ceiling, the deny rules and the hook.
pub(crate) fn lean_settings(launch: &LeanLaunch<'_>) -> Value {
    let milliseconds = (launch.policy.bash_timeout_seconds * 1000).to_string();
    let deny: Vec<&str> = DENIED_TOOLS
        .iter()
        .chain(DENIED_BASH.iter())
        .copied()
        .collect();
    json!({
        "env": {
            "BASH_DEFAULT_TIMEOUT_MS": milliseconds,
            "BASH_MAX_TIMEOUT_MS": milliseconds,
            "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS": "1",
        },
        "permissions": {"deny": deny},
        "hooks": {"PreToolUse": [{
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": hook_command(launch),
                "timeout": HOOK_TIMEOUT_SECONDS,
            }],
        }]},
    })
}

fn object_at<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("{label} {key} must be an object"))
}

fn array_at<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<&'a mut Vec<Value>, String> {
    object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| format!("{label} {key} must be an array"))
}

/// Merge the lean settings into an operator's own settings: the lean environment wins, deny lists
/// are united, and the lean PreToolUse hook is appended to any the operator has.
pub(crate) fn merge_settings(base: Value, lean: &Value) -> Result<Value, String> {
    let label = "the profile's --settings";
    let Value::Object(mut merged) = base else {
        return Err(format!("{label} must be a JSON object"));
    };
    if let Some(environment) = lean["env"].as_object() {
        let target = object_at(&mut merged, "env", label)?;
        for (name, value) in environment {
            target.insert(name.clone(), value.clone());
        }
    }
    if let Some(deny) = lean["permissions"]["deny"].as_array() {
        let permissions = object_at(&mut merged, "permissions", label)?;
        let target = array_at(permissions, "deny", label)?;
        for rule in deny {
            if !target.contains(rule) {
                target.push(rule.clone());
            }
        }
    }
    if let Some(entries) = lean["hooks"]["PreToolUse"].as_array() {
        let hooks = object_at(&mut merged, "hooks", label)?;
        let target = array_at(hooks, "PreToolUse", label)?;
        target.extend(entries.iter().cloned());
    }
    Ok(Value::Object(merged))
}

fn read_operator_settings(value: &str) -> Result<Value, String> {
    if value.trim_start().starts_with('{') {
        return serde_json::from_str(value)
            .map_err(|error| format!("the profile's inline --settings is not JSON: {error}"));
    }
    let path = Path::new(value);
    let size = fs::metadata(path)
        .map_err(|error| format!("cannot read the profile's --settings {value}: {error}"))?
        .len();
    if size > MAX_SETTINGS_BYTES {
        return Err(format!(
            "the profile's --settings {value} is larger than {MAX_SETTINGS_BYTES} bytes"
        ));
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read the profile's --settings {value}: {error}"))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("the profile's --settings {value} is not JSON: {error}"))
}

/// Render a lean launch: write the agent's settings file (the operator's own `--settings`, if the
/// arguments carry one, merged with the lean settings) and return the arguments with that one
/// `--settings` in place of the operator's. Idempotent: arguments that already name this agent's
/// rendered file are rendered again from scratch, never merged into themselves.
pub(crate) fn apply_launch(
    arguments: &[String],
    launch: &LeanLaunch<'_>,
) -> Result<Vec<String>, String> {
    let ours = settings_path(launch.registry, launch.agent);
    let ours_text = ours.display().to_string();
    let mut kept = Vec::with_capacity(arguments.len() + 2);
    let mut operator: Option<String> = None;
    let mut position: Option<usize> = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        let value = if let Some(value) = argument.strip_prefix("--settings=") {
            index += 1;
            Some(value.to_owned())
        } else if argument == "--settings" {
            let value = arguments
                .get(index + 1)
                .cloned()
                .ok_or_else(|| "--settings in the launch arguments has no value".to_owned())?;
            index += 2;
            Some(value)
        } else {
            kept.push(argument.clone());
            index += 1;
            None
        };
        if let Some(value) = value {
            if value == ours_text {
                // Re-rendered in the same place, so a relaunch keeps its argument order.
                position.get_or_insert(kept.len());
                continue;
            }
            if operator.is_some() {
                return Err(
                    "the launch arguments carry more than one --settings; a lean launch merges \
                     exactly one"
                        .to_owned(),
                );
            }
            operator = Some(value);
        }
    }
    let base = match operator.as_deref() {
        Some(value) => read_operator_settings(value)?,
        None => Value::Object(Map::new()),
    };
    let merged = merge_settings(base, &lean_settings(launch))?;
    let directory = launch.registry.join(SETTINGS_DIRECTORY);
    crate::agent::create_private_directory(&directory, "lean settings", false, true)
        .map_err(|error| error.to_string())?;
    crate::agent::atomic_json(&ours, &merged).map_err(|error| error.to_string())?;
    let at = position.unwrap_or(kept.len());
    kept.splice(at..at, ["--settings".to_owned(), ours_text]);
    Ok(kept)
}

/// `arguments` without what a lean launch of `agent` renders: its `--settings` and the
/// agentctl-owned strict-MCP arguments. A relaunch verification compares the rest, since a
/// recorded or refreshed launch spec need not hold them.
pub(crate) fn without_rendered(arguments: &[String], registry: &Path, agent: &str) -> Vec<String> {
    let ours = settings_path(registry, agent).display().to_string();
    let empty_mcp = registry.join("mcp-empty.json").display().to_string();
    let mut kept = Vec::with_capacity(arguments.len());
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        let pair_value = arguments.get(index + 1);
        if (argument == "--settings" && pair_value == Some(&ours))
            || (argument == "--mcp-config" && pair_value == Some(&empty_mcp))
        {
            index += 2;
            continue;
        }
        if argument == "--strict-mcp-config"
            || argument.strip_prefix("--settings=") == Some(ours.as_str())
        {
            index += 1;
            continue;
        }
        kept.push(argument.clone());
        index += 1;
    }
    kept
}

/// What the hook knows about the agent it guards, from its command line.
#[derive(Clone, Debug)]
pub(crate) struct HookPolicy {
    /// The lean agent's name.
    pub(crate) agent: String,
    /// The registry the agent belongs to, as the hook was given it.
    pub(crate) registry: String,
    /// Longest a Bash call may run, in seconds.
    pub(crate) bash_timeout_seconds: u64,
    /// Extra agents `send` and `interrupt` may name.
    pub(crate) lanes: Vec<String>,
    /// Operator-added read-only commands, split into words.
    pub(crate) extra_commands: Vec<Vec<String>>,
}

impl HookPolicy {
    fn hands(&self) -> String {
        self.lanes
            .iter()
            .find(|lane| lane.ends_with("-hands"))
            .cloned()
            .unwrap_or_else(|| format!("{}-hands", self.agent))
    }

    fn way_out(&self) -> String {
        format!(
            "Hand long, writing or network work to your hands lane: agentctl send {} \
             --queue-only '<request; reply with agentctl send --to-manager>'",
            self.hands()
        )
    }
}

/// The hook's answer for one tool call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Decision {
    /// No decision: the call goes on to Claude Code's other checks.
    Pass,
    /// Refused, with the reason the model reads.
    Deny(String),
}

impl Decision {
    /// The PreToolUse output for this decision, or `None` to print nothing.
    pub(crate) fn output(&self) -> Option<Value> {
        match self {
            Self::Pass => None,
            Self::Deny(reason) => Some(json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }})),
        }
    }
}

/// Decide one PreToolUse call. `own_lane(name)` answers whether agent `name` answers to the lean
/// agent through managed_by edges. An input the hook cannot read is an error, which blocks the
/// call (the hook exits non-zero).
pub(crate) fn decide(
    input: &Value,
    policy: &HookPolicy,
    own_lane: &dyn Fn(&str) -> bool,
) -> Result<Decision, String> {
    let tool = input["tool_name"]
        .as_str()
        .ok_or("the hook input has no tool_name")?;
    if tool == "Bash" {
        return decide_bash(&input["tool_input"], policy, own_lane);
    }
    if PASS_TOOLS.contains(&tool) {
        return Ok(Decision::Pass);
    }
    Ok(Decision::Deny(format!(
        "coordinator-lean: the {tool} tool is not available to a lean coordinator. {}",
        policy.way_out()
    )))
}

fn decide_bash(
    input: &Value,
    policy: &HookPolicy,
    own_lane: &dyn Fn(&str) -> bool,
) -> Result<Decision, String> {
    let command = input["command"]
        .as_str()
        .ok_or("the Bash call has no command")?;
    let limit = policy.bash_timeout_seconds;
    if input["run_in_background"].as_bool() == Some(true) {
        return Ok(Decision::Deny(format!(
            "coordinator-lean: background Bash is off for a lean coordinator. {}",
            policy.way_out()
        )));
    }
    if let Some(timeout) = input["timeout"].as_u64() {
        if timeout > limit * 1000 {
            return Ok(Decision::Deny(format!(
                "coordinator-lean: Bash calls are limited to {limit} s (this one asked for {} ms). \
                 {}",
                timeout,
                policy.way_out()
            )));
        }
    }
    match classify(command, policy, own_lane) {
        Ok(()) => Ok(Decision::Pass),
        Err(reason) => Ok(Decision::Deny(format!(
            "coordinator-lean: {reason}. A lean coordinator runs only short read-only commands \
             (each Bash call is cut at {limit} s), agentctl send --queue-only and interrupt to its \
             own lanes, and agent-teams messaging. {}",
            policy.way_out()
        ))),
    }
}

/// One lexical piece of a command line.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    Word(String),
    /// `;`, `&&`, `||` or a newline: the next command starts.
    Separator,
    /// `|`: the next command reads this one's output.
    Pipe,
}

fn is_metacharacter(character: char) -> bool {
    character.is_whitespace() || "|&;<>()'\"\\`$#{}".contains(character)
}

/// Check a `$` at `chars[index]`: a plain `$NAME`, `${NAME}`, `$?`, `$1` and similar expand to
/// a value only; command substitution, ANSI-C quoting and parameter operators are refused.
fn check_dollar(chars: &[char], index: usize) -> Result<usize, String> {
    match chars.get(index + 1) {
        Some('(') => Err("command substitution $( ) is not allowed".to_owned()),
        Some('[') => Err("$[ ] arithmetic is not allowed".to_owned()),
        Some('\'' | '"') => Err("$'...' and $\"...\" quoting is not allowed".to_owned()),
        Some('{') => {
            let mut end = index + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            let name: String = chars[index + 2..end.min(chars.len())].iter().collect();
            let valid = end < chars.len()
                && !name.is_empty()
                && name
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_');
            if valid {
                Ok(end + 1)
            } else {
                Err("only plain ${NAME} expansion is allowed".to_owned())
            }
        }
        _ => Ok(index + 1),
    }
}

fn redirect_target(chars: &[char], mut index: usize) -> (String, usize) {
    while index < chars.len() && (chars[index] == ' ' || chars[index] == '\t') {
        index += 1;
    }
    let start = index;
    while index < chars.len() && !is_metacharacter(chars[index]) {
        index += 1;
    }
    (chars[start..index].iter().collect(), index)
}

/// Read a redirection whose operator starts at `chars[index]`, with `descriptor` the digits
/// written just before it (if any). Only writes to /dev/null, duplication between descriptors 1
/// and 2, and reading a file are allowed. Returns the index after it.
fn redirection(chars: &[char], index: usize, descriptor: Option<&str>) -> Result<usize, String> {
    let output = chars[index] == '>';
    let mut next = index + 1;
    if let Some(digits) = descriptor {
        if !matches!(digits, "0" | "1" | "2") {
            return Err(format!("redirection of descriptor {digits} is not allowed"));
        }
    }
    if output {
        match chars.get(next) {
            Some('>' | '|') => next += 1,
            Some('&') => {
                let target = chars.get(next + 1).copied();
                return match target {
                    Some('1' | '2')
                        if chars
                            .get(next + 2)
                            .is_none_or(|character| is_metacharacter(*character)) =>
                    {
                        Ok(next + 2)
                    }
                    _ => Err("only >&1 and >&2 duplication is allowed".to_owned()),
                };
            }
            Some('(') => return Err("process substitution >( ) is not allowed".to_owned()),
            _ => {}
        }
        let (target, after) = redirect_target(chars, next);
        if target != "/dev/null" {
            return Err(format!(
                "writing to {} is not allowed (only /dev/null)",
                if target.is_empty() {
                    "a file"
                } else {
                    target.as_str()
                }
            ));
        }
        Ok(after)
    } else {
        match chars.get(next) {
            Some('<') => Err("here-documents and here-strings are not allowed".to_owned()),
            Some('(') => Err("process substitution <( ) is not allowed".to_owned()),
            Some('>' | '&') => Err("this redirection is not allowed".to_owned()),
            _ => {
                let (target, after) = redirect_target(chars, next);
                if target.is_empty() {
                    return Err("an input redirection needs a plain file name".to_owned());
                }
                next = after;
                Ok(next)
            }
        }
    }
}

/// Split a command line into words and separators, refusing every construct the classifier does
/// not model.
fn tokenize(command: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quoted = false;
    let flush =
        |tokens: &mut Vec<Token>, word: &mut String, in_word: &mut bool, quoted: &mut bool| {
            if *in_word {
                tokens.push(Token::Word(std::mem::take(word)));
            }
            *in_word = false;
            *quoted = false;
        };
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        match character {
            ' ' | '\t' => {
                flush(&mut tokens, &mut word, &mut in_word, &mut quoted);
                index += 1;
            }
            '\n' => {
                flush(&mut tokens, &mut word, &mut in_word, &mut quoted);
                tokens.push(Token::Separator);
                index += 1;
            }
            '#' if !in_word => {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
            '\'' => {
                let end = chars[index + 1..]
                    .iter()
                    .position(|character| *character == '\'')
                    .ok_or("an unterminated ' quote")?;
                word.extend(&chars[index + 1..index + 1 + end]);
                in_word = true;
                quoted = true;
                index += end + 2;
            }
            '"' => {
                in_word = true;
                quoted = true;
                index += 1;
                loop {
                    let Some(&inner) = chars.get(index) else {
                        return Err("an unterminated \" quote".to_owned());
                    };
                    match inner {
                        '"' => {
                            index += 1;
                            break;
                        }
                        '\\' => {
                            match chars.get(index + 1) {
                                Some('\n') => {}
                                Some(next @ ('$' | '`' | '"' | '\\')) => word.push(*next),
                                Some(next) => {
                                    word.push('\\');
                                    word.push(*next);
                                }
                                None => return Err("an unterminated \" quote".to_owned()),
                            }
                            index += 2;
                        }
                        '`' => return Err("command substitution ` ` is not allowed".to_owned()),
                        '$' => {
                            let end = check_dollar(&chars, index)?;
                            word.extend(&chars[index..end]);
                            index = end;
                        }
                        _ => {
                            word.push(inner);
                            index += 1;
                        }
                    }
                }
            }
            '\\' => {
                match chars.get(index + 1) {
                    // A line continuation joins the lines and adds nothing to the word.
                    Some('\n') => {}
                    Some(next) => {
                        word.push(*next);
                        in_word = true;
                        quoted = true;
                    }
                    None => return Err("a trailing backslash".to_owned()),
                }
                index += 2;
            }
            '`' => return Err("command substitution ` ` is not allowed".to_owned()),
            '$' => {
                let end = check_dollar(&chars, index)?;
                word.extend(&chars[index..end]);
                in_word = true;
                index = end;
            }
            '|' => {
                flush(&mut tokens, &mut word, &mut in_word, &mut quoted);
                match chars.get(index + 1) {
                    Some('|') => {
                        tokens.push(Token::Separator);
                        index += 2;
                    }
                    Some('&') => return Err("|& is not allowed".to_owned()),
                    _ => {
                        tokens.push(Token::Pipe);
                        index += 1;
                    }
                }
            }
            '&' => {
                flush(&mut tokens, &mut word, &mut in_word, &mut quoted);
                if chars.get(index + 1) == Some(&'&') {
                    tokens.push(Token::Separator);
                    index += 2;
                } else {
                    return Err("running a command in the background (&) is not allowed".to_owned());
                }
            }
            ';' => {
                flush(&mut tokens, &mut word, &mut in_word, &mut quoted);
                if chars.get(index + 1) == Some(&';') {
                    return Err(";; is not allowed".to_owned());
                }
                tokens.push(Token::Separator);
                index += 1;
            }
            '>' | '<' => {
                let descriptor = if in_word
                    && !quoted
                    && !word.is_empty()
                    && word.chars().all(|digit| digit.is_ascii_digit())
                {
                    let digits = std::mem::take(&mut word);
                    in_word = false;
                    Some(digits)
                } else {
                    flush(&mut tokens, &mut word, &mut in_word, &mut quoted);
                    None
                };
                index = redirection(&chars, index, descriptor.as_deref())?;
            }
            '(' | ')' => return Err("subshells and ( ) grouping are not allowed".to_owned()),
            '{' | '}' => {
                return Err("unquoted { } (grouping or brace expansion) is not allowed".to_owned())
            }
            '!' if !in_word => {
                return Err("! negation and history expansion are not allowed".to_owned())
            }
            _ => {
                word.push(character);
                in_word = true;
                index += 1;
            }
        }
    }
    flush(&mut tokens, &mut word, &mut in_word, &mut quoted);
    Ok(tokens)
}

/// Variables a lean command may set for one command (`TZ=... date`).
fn allowed_assignment(word: &str, policy: &HookPolicy) -> Option<bool> {
    let (name, value) = word.split_once('=')?;
    let identifier = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !identifier {
        return None;
    }
    Some(match name {
        "TZ" | "LANG" | "NO_COLOR" => true,
        "AGENTCTL_AGENT" => value == policy.agent,
        _ => name.starts_with("LC_"),
    })
}

/// Check every command of a command line.
fn classify(
    command: &str,
    policy: &HookPolicy,
    own_lane: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    let tokens = tokenize(command)?;
    let mut segment: Vec<String> = Vec::new();
    let mut previous_pipe = false;
    let mut any = false;
    let mut finish = |segment: &mut Vec<String>, piped: bool| -> Result<(), String> {
        if segment.is_empty() {
            if piped {
                return Err("a pipe with no command on one side".to_owned());
            }
            return Ok(());
        }
        any = true;
        let words = std::mem::take(segment);
        check_command(&words, policy, own_lane)
    };
    for token in tokens {
        match token {
            Token::Word(word) => segment.push(word),
            Token::Separator => {
                finish(&mut segment, previous_pipe)?;
                previous_pipe = false;
            }
            Token::Pipe => {
                finish(&mut segment, true)?;
                previous_pipe = true;
            }
        }
    }
    finish(&mut segment, previous_pipe)?;
    if any {
        Ok(())
    } else {
        Err("an empty command".to_owned())
    }
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// Programs that only read, accepted with any arguments.
const READ_ONLY_PROGRAMS: [&str; 32] = [
    "cat", "head", "wc", "ls", "stat", "file", "basename", "dirname", "realpath", "readlink",
    "pwd", "id", "whoami", "echo", "printf", "true", "false", "nproc", "uptime", "df", "du", "cut",
    "tr", "comm", "diff", "jq", "grep", "egrep", "fgrep", "ps", "pgrep", "column",
];

/// Programs with argument rules of their own.
const RULED_PROGRAMS: [&str; 9] = [
    "tail", "sort", "uniq", "rg", "find", "sed", "date", "git", "agentctl",
];

fn short_cluster_has(word: &str, letters: &str) -> bool {
    word.starts_with('-')
        && !word.starts_with("--")
        && word[1..]
            .chars()
            .any(|character| letters.contains(character))
}

fn check_command(
    words: &[String],
    policy: &HookPolicy,
    own_lane: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    let mut start = 0;
    while let Some(word) = words.get(start) {
        match allowed_assignment(word, policy) {
            Some(true) => start += 1,
            Some(false) => {
                return Err(format!(
                    "setting {} for a command is not allowed",
                    word.split('=').next().unwrap_or(word)
                ))
            }
            None => break,
        }
    }
    let words = &words[start..];
    let Some(program) = words.first() else {
        return Err("a variable assignment on its own is not allowed".to_owned());
    };
    for extra in &policy.extra_commands {
        if words.len() >= extra.len()
            && extra
                .iter()
                .zip(words)
                .enumerate()
                .all(|(position, (want, have))| {
                    want == have || (position == 0 && basename(want) == basename(have))
                })
        {
            return Ok(());
        }
    }
    let name = basename(program);
    if program.contains('/') {
        let trusted_directory = ["/usr/bin/", "/bin/", "/usr/local/bin/"]
            .iter()
            .any(|directory| {
                program
                    .strip_prefix(directory)
                    .is_some_and(|rest| !rest.contains('/'))
            });
        let suite_tool = matches!(name, "agentctl" | "agent-teams");
        if !(suite_tool || trusted_directory) {
            return Err(format!(
                "{program} is not a lean read-only command (run programs by name, or from \
                 /usr/bin)"
            ));
        }
    }
    let rest = &words[1..];
    if READ_ONLY_PROGRAMS.contains(&name) {
        return Ok(());
    }
    if !RULED_PROGRAMS.contains(&name) && name != "agent-teams" {
        return Err(format!("{name} is not a lean read-only command"));
    }
    match name {
        "tail" => {
            if rest.iter().any(|word| {
                word.starts_with("--follow") || word == "--retry" || short_cluster_has(word, "fF")
            }) {
                return Err("tail -f / --follow is not allowed (it never ends)".to_owned());
            }
            Ok(())
        }
        "sort" => {
            if rest.iter().any(|word| {
                word.starts_with("--output")
                    || word.starts_with("--compress-program")
                    || short_cluster_has(word, "o")
            }) {
                return Err("sort -o / --output / --compress-program is not allowed".to_owned());
            }
            Ok(())
        }
        "uniq" => {
            let operands = rest.iter().filter(|word| !word.starts_with('-')).count();
            if operands > 1 {
                return Err("uniq with an output file is not allowed".to_owned());
            }
            Ok(())
        }
        "rg" => {
            if rest.iter().any(|word| {
                word.starts_with("--pre")
                    || word == "--search-zip"
                    || word.starts_with("--hostname-bin")
                    || short_cluster_has(word, "z")
            }) {
                return Err(
                    "rg --pre, --search-zip (-z) and --hostname-bin run programs".to_owned(),
                );
            }
            Ok(())
        }
        "find" => {
            const ACTIONS: [&str; 9] = [
                "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0", "-fprintf",
                "-fls",
            ];
            if let Some(action) = rest.iter().find(|word| ACTIONS.contains(&word.as_str())) {
                return Err(format!("find {action} is not allowed"));
            }
            Ok(())
        }
        "sed" => check_sed(rest),
        "date" => {
            if rest
                .iter()
                .any(|word| word.starts_with("--set") || short_cluster_has(word, "s"))
            {
                return Err("date -s / --set is not allowed".to_owned());
            }
            Ok(())
        }
        "git" => check_git(rest),
        "agentctl" => check_agentctl(rest, policy, own_lane),
        "agent-teams" => check_agent_teams(rest),
        _ => Err(format!("{name} is not a lean read-only command")),
    }
}

fn sed_range_script(script: &str) -> bool {
    let address = |text: &str| {
        text == "$" || (!text.is_empty() && text.chars().all(|digit| digit.is_ascii_digit()))
    };
    script
        .split([';', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .all(|part| {
            let Some(range) = part.strip_suffix('p') else {
                return false;
            };
            let range = range.trim();
            match range.split_once(',') {
                None => address(range),
                Some((first, second)) => {
                    let second = second.trim();
                    address(first.trim())
                        && (address(second)
                            || second.strip_prefix('+').is_some_and(|count| {
                                !count.is_empty() && count.chars().all(|d| d.is_ascii_digit())
                            }))
                }
            }
        })
}

fn check_sed(rest: &[String]) -> Result<(), String> {
    let mut quiet = false;
    let mut scripts: Vec<&str> = Vec::new();
    let mut operands: Vec<&str> = Vec::new();
    let mut index = 0;
    while index < rest.len() {
        let word = rest[index].as_str();
        if word == "--quiet" || word == "--silent" {
            quiet = true;
        } else if word.starts_with("--in-place") || word.starts_with("--file") {
            return Err("sed -i / --in-place and -f / --file are not allowed".to_owned());
        } else if let Some(script) = word.strip_prefix("--expression=") {
            scripts.push(script);
        } else if word == "--expression" || word == "-e" {
            let script = rest.get(index + 1).ok_or("sed -e needs a script")?;
            scripts.push(script);
            index += 1;
        } else if word.starts_with("--") {
            if !matches!(
                word,
                "--regexp-extended" | "--null-data" | "--separate" | "--posix"
            ) {
                return Err(format!("sed option {word} is not allowed"));
            }
        } else if word.starts_with('-') && word.len() > 1 {
            for letter in word[1..].chars() {
                match letter {
                    'n' => quiet = true,
                    'E' | 'r' | 's' | 'z' | 'u' => {}
                    _ => return Err(format!("sed option -{letter} is not allowed")),
                }
            }
        } else {
            operands.push(word);
        }
        index += 1;
    }
    if scripts.is_empty() {
        if operands.is_empty() {
            return Err("sed needs a script".to_owned());
        }
        scripts.push(operands.remove(0));
    }
    if !quiet {
        return Err(
            "sed is allowed only with -n and line-range p scripts (sed -n '10,20p' FILE)"
                .to_owned(),
        );
    }
    if let Some(bad) = scripts.iter().find(|script| !sed_range_script(script)) {
        return Err(format!(
            "sed script {bad:?} is not a line-range print (use forms like 10,20p or 5p)"
        ));
    }
    Ok(())
}

fn check_git(rest: &[String]) -> Result<(), String> {
    let mut index = 0;
    while let Some(word) = rest.get(index) {
        if !word.starts_with('-') {
            break;
        }
        match word.as_str() {
            "-C" => {
                if rest.get(index + 1).is_none() {
                    return Err("git -C needs a directory".to_owned());
                }
                index += 2;
            }
            "--no-pager" | "-P" | "--no-optional-locks" | "--literal-pathspecs" => index += 1,
            _ => return Err(format!("git option {word} is not allowed")),
        }
    }
    let subcommand = rest
        .get(index)
        .map(String::as_str)
        .ok_or("git needs a subcommand")?;
    let arguments = &rest[index + 1..];
    if let Some(bad) = arguments.iter().find(|word| {
        word.starts_with("--output")
            || word.starts_with("--ext-diff")
            || word.starts_with("--upload-pack")
            || word.starts_with("--exec")
            || word.starts_with("--open-files-in-pager")
            || word.as_str() == "-O"
    }) {
        return Err(format!("git {subcommand} {bad} is not allowed"));
    }
    let first = arguments.first().map(String::as_str);
    let allowed = match subcommand {
        "log" | "show" | "status" | "diff" | "rev-parse" | "ls-files" | "ls-tree" | "cat-file"
        | "blame" | "describe" | "merge-base" | "shortlog" | "grep" | "rev-list" | "name-rev"
        | "show-ref" | "for-each-ref" | "count-objects" | "whatchanged" | "cherry" => true,
        "branch" => {
            const WRITES: [&str; 14] = [
                "-d",
                "-D",
                "-m",
                "-M",
                "-c",
                "-C",
                "-f",
                "-u",
                "--delete",
                "--move",
                "--copy",
                "--force",
                "--edit-description",
                "--unset-upstream",
            ];
            let listing = arguments.iter().any(|word| {
                matches!(
                    word.as_str(),
                    "--list"
                        | "-l"
                        | "--contains"
                        | "--no-contains"
                        | "--merged"
                        | "--no-merged"
                        | "--points-at"
                )
            });
            let positional = arguments.iter().any(|word| !word.starts_with('-'));
            !arguments
                .iter()
                .any(|word| WRITES.contains(&word.as_str()) || word.starts_with("--set-upstream"))
                && (listing || !positional)
        }
        "worktree" => first == Some("list"),
        "stash" => matches!(first, Some("list" | "show")),
        "remote" => match first {
            None => true,
            Some("get-url") => true,
            Some(_) => arguments
                .iter()
                .all(|word| matches!(word.as_str(), "-v" | "--verbose")),
        },
        "config" => {
            let reads = arguments.iter().any(|word| {
                matches!(
                    word.as_str(),
                    "--get" | "--get-all" | "--get-regexp" | "--list" | "-l"
                )
            });
            let writes = arguments.iter().any(|word| {
                matches!(
                    word.as_str(),
                    "--add"
                        | "--unset"
                        | "--unset-all"
                        | "--replace-all"
                        | "--edit"
                        | "-e"
                        | "--rename-section"
                        | "--remove-section"
                )
            });
            reads && !writes
        }
        "reflog" => match first {
            None => true,
            Some(word) => word == "show" || word.starts_with('-'),
        },
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "git {subcommand} is not a read-only git command here"
        ))
    }
}

/// `agentctl send` options that take a value.
const SEND_VALUE_OPTIONS: [&str; 10] = [
    "--file",
    "--message-id",
    "--model",
    "--ttl",
    "--supersede-key",
    "--ready-timeout",
    "--working-timeout",
    "--max-attempts",
    "--boundary-wait",
    "--deadline",
];

fn check_agentctl(
    rest: &[String],
    policy: &HookPolicy,
    own_lane: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    let mut index = 0;
    while let Some(word) = rest.get(index) {
        if !word.starts_with('-') {
            break;
        }
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (word.as_str(), None),
        };
        match name {
            "--registry" => {
                let value = match inline {
                    Some(value) => {
                        index += 1;
                        value
                    }
                    None => {
                        index += 2;
                        rest.get(index - 1)
                            .map(String::as_str)
                            .ok_or("agentctl --registry needs a value")?
                    }
                };
                if value != policy.registry {
                    return Err(
                        "agentctl --registry may name only this agent's own registry".to_owned(),
                    );
                }
            }
            "--herdr-bin" => index += if inline.is_some() { 1 } else { 2 },
            "--help" | "-h" | "--version" | "-V" => return Ok(()),
            _ => return Err(format!("agentctl option {word} is not allowed")),
        }
    }
    let subcommand = rest
        .get(index)
        .map(String::as_str)
        .ok_or("agentctl needs a subcommand")?;
    let arguments = &rest[index + 1..];
    match subcommand {
        "status" | "list" | "read" | "capabilities" | "profiles" | "tree" | "approvals"
        | "requests" | "help" | "quickstart" | "userguide" => Ok(()),
        "tail" => {
            if arguments
                .iter()
                .any(|word| word == "--follow" || short_cluster_has(word, "f"))
            {
                Err("agentctl tail --follow is not allowed (it never ends)".to_owned())
            } else {
                Ok(())
            }
        }
        "chat" => match arguments.first().map(String::as_str) {
            Some("sent" | "thread" | "help" | "status" | "health" | "--help" | "-h") => Ok(()),
            Some(other) => Err(format!(
                "agentctl chat {other} is not available to a lean coordinator (reply by \
                 CHAT_REPLY block)"
            )),
            None => Err("agentctl chat needs a subcommand".to_owned()),
        },
        "send" => check_send(arguments, policy, own_lane),
        "interrupt" => {
            let target = arguments
                .iter()
                .find(|word| !word.starts_with('-'))
                .ok_or("agentctl interrupt needs an agent name")?;
            require_own_lane(target, policy, own_lane)
        }
        _ => Err(format!(
            "agentctl {subcommand} is not available to a lean coordinator"
        )),
    }
}

fn require_own_lane(
    target: &str,
    policy: &HookPolicy,
    own_lane: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    if policy.lanes.iter().any(|lane| lane == target) || own_lane(target) {
        Ok(())
    } else {
        Err(format!(
            "{target} is not one of your lanes (no managed_by edge leads from it to {}, and the \
             profile's coordinator_lean.lanes does not list it); use --to-manager for your \
             manager and agent-teams for other teams",
            policy.agent
        ))
    }
}

fn check_send(
    arguments: &[String],
    policy: &HookPolicy,
    own_lane: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    let mut queue_only = false;
    let mut relationship = false;
    let mut positionals: Vec<&str> = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let word = arguments[index].as_str();
        if word == "--" {
            positionals.extend(arguments[index + 1..].iter().map(String::as_str));
            break;
        }
        if let Some(option) = word.strip_prefix("--") {
            let (name, inline) = match option.split_once('=') {
                Some((name, _)) => (name, true),
                None => (option, false),
            };
            let name = format!("--{name}");
            match name.as_str() {
                "--queue-only" => queue_only = true,
                "--to-manager" | "--to-children" => relationship = true,
                "--deliver-while-working" => {}
                "--urgent" => {
                    return Err(
                        "agentctl send --urgent waits up to a minute for the lane; use \
                         agentctl interrupt for a runaway lane"
                            .to_owned(),
                    )
                }
                option if SEND_VALUE_OPTIONS.contains(&option) => {
                    if !inline {
                        index += 1;
                    }
                }
                other => return Err(format!("agentctl send option {other} is not allowed")),
            }
        } else if word.starts_with('-') && word.len() > 1 {
            return Err(format!("agentctl send option {word} is not allowed"));
        } else {
            positionals.push(word);
        }
        index += 1;
    }
    if !queue_only {
        return Err(
            "agentctl send needs --queue-only here: it returns once the message is queued, while \
             a send that waits for the lane can outlast the Bash time limit and tempt a duplicate"
                .to_owned(),
        );
    }
    if relationship {
        return Ok(());
    }
    let target = positionals
        .first()
        .ok_or("agentctl send needs an agent name")?;
    require_own_lane(target, policy, own_lane)
}

fn check_agent_teams(rest: &[String]) -> Result<(), String> {
    let mut index = 0;
    while let Some(word) = rest.get(index) {
        match word.as_str() {
            "--teams" | "--routes" => index += 2,
            word if word.starts_with("--teams=") || word.starts_with("--routes=") => index += 1,
            "--help" | "-h" => return Ok(()),
            _ => break,
        }
    }
    match rest.get(index).map(String::as_str) {
        Some(
            "send" | "reply" | "status" | "outbox" | "show" | "check" | "coordinators" | "seen"
            | "receipt" | "help",
        ) => Ok(()),
        Some(other) => Err(format!(
            "agent-teams {other} is not available to a lean coordinator"
        )),
        None => Err("agent-teams needs a subcommand".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> HookPolicy {
        HookPolicy {
            agent: "lead".to_owned(),
            registry: "/team/.agentctl".to_owned(),
            bash_timeout_seconds: 30,
            lanes: vec!["lead-hands".to_owned()],
            extra_commands: vec![vec!["/opt/tool/report".to_owned(), "prompts".to_owned()]],
        }
    }

    fn own(name: &str) -> bool {
        name == "lane-a" || name == "lane-b"
    }

    fn bash(command: &str) -> Decision {
        decide(
            &json!({"tool_name": "Bash", "tool_input": {"command": command}}),
            &policy(),
            &own,
        )
        .unwrap()
    }

    fn allowed(command: &str) {
        assert_eq!(bash(command), Decision::Pass, "{command}");
    }

    fn refused(command: &str) -> String {
        match bash(command) {
            Decision::Deny(reason) => reason,
            Decision::Pass => panic!("{command} was allowed"),
        }
    }

    #[test]
    fn short_read_only_commands_pass() {
        for command in [
            "cat notes.md",
            "sed -n '10,40p' src/main.rs",
            "sed -n -e 5p -e '$p' a.txt",
            "head -n 20 log.txt | tail -n 5",
            "rg -n 'fn main' rs/ 2>/dev/null | head",
            "grep -c foo a b >/dev/null 2>&1",
            "wc -l < notes.md",
            "ls -la ~/x && stat -c %s f",
            "git -C /repo log --oneline -5",
            "git status --short; git diff --stat HEAD~1",
            "git branch --list 'cl-*'",
            "git branch -a",
            "git worktree list",
            "git config --get user.name",
            "TZ=America/New_York date '+%-I:%M %p ET'",
            "find . -name '*.rs' -newer x",
            "sort -u a | uniq -c",
            "jq '.items[] | {name}' f.json",
            "echo \"$HOME/${USER}\"",
            "agentctl status lane-a",
            "/team/bin/agentctl --registry /team/.agentctl read lane-b",
            "agentctl tail lane-a",
            "agentctl chat sent 7591",
            "agentctl chat thread 7591",
            "agentctl send lane-a --queue-only 'H17: run the gate\nreply with --to-manager'",
            "agentctl send --queue-only --to-manager 'done'",
            "agentctl send lead-hands --queue-only --file brief.md",
            "agentctl interrupt lane-b",
            "AGENTCTL_AGENT=lead agentctl send lane-a --queue-only hi",
            "agent-teams send lead@other --text 'status?'",
            "agent-teams reply 42 --text ok",
            "/opt/tool/report prompts --hours 2",
            "cat a # a trailing comment",
            "/usr/bin/cat x",
        ] {
            allowed(command);
        }
    }

    #[test]
    fn writes_network_and_long_running_commands_are_refused() {
        for command in [
            "touch x",
            "rm -rf build",
            "echo hi > notes.md",
            "echo hi >> notes.md",
            "echo hi | tee notes.md",
            "cat a 1>out",
            "sed -i 's/a/b/' f",
            "sed -ni '1p' f",
            "sed -n '1w out' f",
            "sed -n 's/a/b/p' f",
            "sed 's/a/b/' f",
            "find . -delete",
            "find . -name x -exec rm {} ;",
            "tail -f log",
            "tail -n5 -F log",
            "sort -o out in",
            "uniq in out",
            "rg --pre cat x",
            "git -c core.pager=sh log",
            "git fetch origin",
            "git push origin main",
            "git commit -m x",
            "git branch -D old",
            "git branch newname",
            "git diff --output=f",
            "git config user.name x",
            "git stash pop",
            "curl https://example.com",
            "sleep 31",
            "python3 script.py",
            "cargo build",
            "bash -c 'ls'",
            "agentctl start worker --profile p",
            "agentctl stop lane-a",
            "agentctl tail --follow lane-a",
            "agentctl chat reply 7591 --file r.md",
            "agentctl send lane-a 'no queue-only'",
            "agentctl send stranger --queue-only hi",
            "agentctl send lane-a --queue-only --urgent hi",
            "agentctl interrupt stranger",
            "agentctl --registry /other/.agentctl send lane-a --queue-only hi",
            "AGENTCTL_AGENT=someone agentctl send lane-a --queue-only hi",
            "AGENTCTL_REGISTRY=/other agentctl status x",
            "PATH=/tmp ls",
            "X=1",
            "agent-teams resolve 42 --fence",
            "/tmp/x/cat a",
            "./script",
        ] {
            refused(command);
        }
    }

    #[test]
    fn shell_constructs_outside_the_model_are_refused() {
        for (command, why) in [
            ("echo $(rm -rf x)", "command substitution"),
            ("echo \"$(id)\"", "command substitution"),
            ("echo `id`", "command substitution"),
            ("cat <(ls)", "process substitution"),
            ("cat <<EOF\nx\nEOF", "here-documents"),
            ("ls &", "background"),
            ("(cd x; ls)", "subshells"),
            ("{ ls; }", "grouping"),
            ("find . -{delete,print}", "brace"),
            ("! ls", "negation"),
            ("echo ${X:-$(id)}", "plain ${NAME}"),
            ("find . $'-delete'", "quoting"),
            ("echo 'open", "unterminated"),
            ("cat a 3>x", "descriptor"),
            ("ls |& cat", "|&"),
            ("ls | ", "pipe"),
        ] {
            let reason = refused(command);
            assert!(reason.contains(why), "{command}: {reason}");
        }
    }

    #[test]
    fn compound_commands_need_every_part_allowed() {
        allowed("git log -1 && git status");
        refused("git log -1 && rm x");
        refused("cat a; touch b");
        refused("cat a || python3 x");
        refused("ls\ntouch x");
        refused("cat a | sh");
    }

    #[test]
    fn timeouts_and_background_are_refused_before_classification() {
        let decision = decide(
            &json!({"tool_name": "Bash", "tool_input": {"command": "ls", "timeout": 120000}}),
            &policy(),
            &own,
        )
        .unwrap();
        let Decision::Deny(reason) = decision else {
            panic!("long timeout allowed")
        };
        assert!(reason.contains("limited to 30 s"), "{reason}");
        assert!(
            reason.contains("agentctl send lead-hands --queue-only"),
            "{reason}"
        );
        assert_eq!(
            decide(
                &json!({"tool_name": "Bash", "tool_input": {"command": "ls", "timeout": 30000}}),
                &policy(),
                &own,
            )
            .unwrap(),
            Decision::Pass
        );
        let decision = decide(
            &json!({"tool_name": "Bash", "tool_input": {"command": "ls", "run_in_background": true}}),
            &policy(),
            &own,
        )
        .unwrap();
        assert!(matches!(decision, Decision::Deny(reason) if reason.contains("background")));
    }

    #[test]
    fn tools_pass_or_are_refused_by_name_and_bad_input_is_an_error() {
        for tool in PASS_TOOLS {
            assert_eq!(
                decide(
                    &json!({"tool_name": tool, "tool_input": {}}),
                    &policy(),
                    &own
                )
                .unwrap(),
                Decision::Pass
            );
        }
        for tool in [
            "Write",
            "Edit",
            "Task",
            "WebFetch",
            "mcp__x__y",
            "SomethingNew",
        ] {
            let decision = decide(
                &json!({"tool_name": tool, "tool_input": {}}),
                &policy(),
                &own,
            )
            .unwrap();
            assert!(matches!(decision, Decision::Deny(_)), "{tool}");
        }
        assert!(decide(&json!({"tool_input": {}}), &policy(), &own).is_err());
        assert!(decide(
            &json!({"tool_name": "Bash", "tool_input": {}}),
            &policy(),
            &own
        )
        .is_err());
    }

    #[test]
    fn the_deny_output_is_the_pre_tool_use_shape() {
        let output = Decision::Deny("no".to_owned()).output().unwrap();
        assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            output["hookSpecificOutput"]["permissionDecisionReason"],
            "no"
        );
        assert!(Decision::Pass.output().is_none());
    }

    #[test]
    fn policies_parse_with_defaults_and_refuse_mistakes() {
        assert_eq!(policy_from_value(None, "p").unwrap(), None);
        assert_eq!(policy_from_value(Some(&json!(false)), "p").unwrap(), None);
        assert_eq!(
            policy_from_value(Some(&json!(true)), "p").unwrap(),
            Some(LeanPolicy::default())
        );
        let parsed = policy_from_value(
            Some(
                &json!({"bash_timeout_seconds": 20, "extra_commands": ["/opt/t report"],
                          "lanes": ["lead-hands"]}),
            ),
            "p",
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.bash_timeout_seconds, 20);
        assert_eq!(parsed.extra_commands, vec!["/opt/t report".to_owned()]);
        assert_eq!(parsed.lanes, vec!["lead-hands".to_owned()]);
        for bad in [
            json!("yes"),
            json!({"bash_timeout_seconds": 5}),
            json!({"bash_timeout_seconds": 61}),
            json!({"bash_timeout_seconds": "30"}),
            json!({"extra_commands": ["rm -rf; x"]}),
            json!({"extra_commands": "x"}),
            json!({"lanes": ["Bad Name"]}),
            json!({"lanes": ["a", "a"]}),
            json!({"other": 1}),
        ] {
            assert!(policy_from_value(Some(&bad), "p").is_err(), "{bad}");
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agentctl-lean-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::agent::create_private_directory(&root, "test registry", true, false).unwrap();
        root
    }

    #[test]
    fn a_launch_renders_one_settings_file_and_merges_the_operators() {
        let registry = scratch("render");
        let operator = registry.join("operator.json");
        fs::write(
            &operator,
            r#"{"env": {"KEEP": "1", "BASH_MAX_TIMEOUT_MS": "600000"},
                "permissions": {"deny": ["Bash(gh:*)"], "allow": ["Read"]},
                "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": []}], "Stop": []}}"#,
        )
        .unwrap();
        let policy = LeanPolicy {
            bash_timeout_seconds: 25,
            extra_commands: vec!["/opt/t report".to_owned()],
            lanes: vec!["lead-hands".to_owned()],
        };
        let launch = LeanLaunch {
            policy: &policy,
            agentctl: Path::new("/snap/bin/agentctl"),
            registry: &registry,
            agent: "lead",
        };
        let arguments: Vec<String> = [
            "--dangerously-skip-permissions",
            "--settings",
            operator.to_str().unwrap(),
            "--model",
            "m",
        ]
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect();
        let rendered = apply_launch(&arguments, &launch).unwrap();
        let path = settings_path(&registry, "lead");
        assert_eq!(
            rendered,
            vec![
                "--dangerously-skip-permissions".to_owned(),
                "--model".to_owned(),
                "m".to_owned(),
                "--settings".to_owned(),
                path.display().to_string(),
            ]
        );
        let settings: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["env"]["KEEP"], "1");
        assert_eq!(settings["env"]["BASH_MAX_TIMEOUT_MS"], "25000");
        assert_eq!(settings["env"]["BASH_DEFAULT_TIMEOUT_MS"], "25000");
        assert_eq!(settings["env"]["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"], "1");
        let deny = settings["permissions"]["deny"].as_array().unwrap();
        assert_eq!(deny[0], "Bash(gh:*)");
        assert!(deny.contains(&json!("Write")) && deny.contains(&json!("Bash(sudo:*)")));
        assert_eq!(settings["permissions"]["allow"], json!(["Read"]));
        let hooks = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(hooks.len(), 2);
        let command = hooks[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(
            command.starts_with("'/snap/bin/agentctl' '--registry' '"),
            "{command}"
        );
        assert!(command.contains("'lean-hook' '--agent' 'lead' '--bash-timeout' '25'"));
        assert!(command.contains("'--lane' 'lead-hands' '--extra-command' '/opt/t report'"));
        assert!(command.ends_with(" || exit 2"), "{command}");
        assert_eq!(settings["hooks"]["Stop"], json!([]));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Rendering the rendered arguments again never merges the file into itself.
        let again = apply_launch(&rendered, &launch).unwrap();
        assert_eq!(again, rendered);
        let settings: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        fs::remove_dir_all(&registry).unwrap();
    }

    #[test]
    fn inline_settings_merge_and_two_operator_settings_refuse() {
        let registry = scratch("inline");
        let policy = LeanPolicy::default();
        let launch = LeanLaunch {
            policy: &policy,
            agentctl: Path::new("/a/agentctl"),
            registry: &registry,
            agent: "lead",
        };
        let rendered =
            apply_launch(&["--settings={\"env\":{\"A\":\"b\"}}".to_owned()], &launch).unwrap();
        assert_eq!(rendered.len(), 2);
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(settings_path(&registry, "lead")).unwrap())
                .unwrap();
        assert_eq!(settings["env"]["A"], "b");
        assert_eq!(settings["env"]["BASH_MAX_TIMEOUT_MS"], "30000");
        assert!(apply_launch(
            &[
                "--settings={}".to_owned(),
                "--settings".to_owned(),
                "{}".to_owned()
            ],
            &launch
        )
        .is_err());
        assert!(apply_launch(&["--settings=[1]".to_owned()], &launch).is_err());
        assert!(apply_launch(&["--settings".to_owned()], &launch).is_err());
        fs::remove_dir_all(&registry).unwrap();
    }

    #[test]
    fn rendered_arguments_are_stripped_for_comparison_and_kept_in_place() {
        let registry = scratch("strip");
        let policy = LeanPolicy::default();
        let launch = LeanLaunch {
            policy: &policy,
            agentctl: Path::new("/a/agentctl"),
            registry: &registry,
            agent: "lead",
        };
        let ours = settings_path(&registry, "lead").display().to_string();
        let empty = registry.join("mcp-empty.json").display().to_string();
        let recorded: Vec<String> = [
            "--resume",
            "s",
            "--settings",
            &ours,
            "--strict-mcp-config",
            "--mcp-config",
            &empty,
            "--model",
            "m",
        ]
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect();
        assert_eq!(
            without_rendered(&recorded, &registry, "lead"),
            vec!["--resume", "s", "--model", "m"]
        );
        // Another agent's settings and another MCP config are kept.
        let other = vec!["--settings".to_owned(), "/x.json".to_owned()];
        assert_eq!(without_rendered(&other, &registry, "lead"), other);
        let rendered = apply_launch(&recorded, &launch).unwrap();
        assert_eq!(rendered, recorded, "re-rendering keeps the argument order");
        fs::remove_dir_all(&registry).unwrap();
    }

    #[test]
    fn quoting_survives_single_quotes_in_paths() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
}
