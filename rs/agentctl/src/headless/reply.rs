//! Messages a headless agent sends through its shell tool, recognised so the event log can show
//! them as `reply.sent` with their text.
//!
//! The agent replies to chat with `agentctl chat reply`, answers team messages with
//! `agent-teams reply`, and messages agents with `agentctl send` or `agent-teams send`. Those
//! commands do the sending; the wrapper only **describes** them. It recognises the command in a
//! `Bash` tool call and, once the call succeeds, records the text the command sent: the inline
//! `--text` (or the positional prompt of `agentctl send`), a here-document fed to `--file -`, or
//! the named file read back after the call. Nothing here runs or repeats a command.

use std::path::Path;

/// One recognised send.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplyCall {
    /// `chat` (chat reply), `team` (agent-teams), or `agent` (agentctl send).
    pub channel: String,
    /// Who it goes to: the request key, team message ID or target, or agent name.
    pub to: String,
    /// The text, when the command line carries it.
    pub text: Option<String>,
    /// The file the text is read from, when it names one.
    pub file: Option<String>,
}

/// Largest reply text kept in an event; `chat reply` refuses more than 30,000 bytes.
pub const REPLY_BYTES: usize = 32 * 1024;

/// Shell words of `command`, with operators (`;`, `&&`, `||`, `|`, newline) as their own words
/// and here-document bodies attached to the `<<` operator as `<<\0BODY`. Quotes are removed; a
/// `$(...)` inside a word is kept verbatim. Good enough to recognise a command line, not a shell.
fn words(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut pending_heredocs: Vec<(usize, String, bool)> = Vec::new();
    let mut i = 0;
    let flush = |word: &mut String, in_word: &mut bool, words: &mut Vec<String>| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                in_word = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '"' => {
                in_word = true;
                i += 1;
                let mut depth = 0usize;
                while i < chars.len() && (chars[i] != '"' || depth > 0) {
                    if chars[i] == '\\' && depth == 0 && i + 1 < chars.len() {
                        let next = chars[i + 1];
                        if matches!(next, '"' | '\\' | '$' | '`') {
                            word.push(next);
                            i += 2;
                            continue;
                        }
                        if next == '\n' {
                            i += 2;
                            continue;
                        }
                    }
                    if chars[i] == '$' && chars.get(i + 1) == Some(&'(') {
                        depth += 1;
                        word.push('$');
                        word.push('(');
                        i += 2;
                        continue;
                    }
                    if chars[i] == ')' && depth > 0 {
                        depth -= 1;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
            }
            '\\' if i + 1 < chars.len() => {
                if chars[i + 1] != '\n' {
                    in_word = true;
                    word.push(chars[i + 1]);
                }
                i += 1;
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                in_word = true;
                let mut depth = 0usize;
                while i < chars.len() {
                    word.push(chars[i]);
                    if chars[i] == '(' {
                        depth += 1;
                    } else if chars[i] == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    i += 1;
                }
            }
            ' ' | '\t' => flush(&mut word, &mut in_word, &mut words),
            '\n' => {
                flush(&mut word, &mut in_word, &mut words);
                // Here-document bodies start on the line after their operator.
                for (index, delimiter, strip_tabs) in std::mem::take(&mut pending_heredocs) {
                    let mut body = Vec::new();
                    loop {
                        let start = i + 1;
                        if start > chars.len() {
                            break;
                        }
                        let end = chars[start..]
                            .iter()
                            .position(|c| *c == '\n')
                            .map_or(chars.len(), |offset| start + offset);
                        let line: String = chars[start..end].iter().collect();
                        i = end;
                        let candidate = if strip_tabs {
                            line.trim_start_matches('\t')
                        } else {
                            line.as_str()
                        };
                        if candidate == delimiter || end >= chars.len() {
                            if candidate != delimiter {
                                body.push(line);
                            }
                            break;
                        }
                        body.push(candidate.to_owned());
                    }
                    if let Some(slot) = words.get_mut(index) {
                        *slot = format!("<<\0{}", body.join("\n"));
                    }
                }
                words.push("\n".to_owned());
            }
            ';' | '|' | '&' => {
                flush(&mut word, &mut in_word, &mut words);
                let mut operator = c.to_string();
                if chars.get(i + 1) == Some(&c) {
                    operator.push(c);
                    i += 1;
                }
                words.push(operator);
            }
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) != Some(&'<') => {
                flush(&mut word, &mut in_word, &mut words);
                i += 2;
                let strip_tabs = chars.get(i) == Some(&'-');
                if strip_tabs {
                    i += 1;
                }
                while chars.get(i).is_some_and(|c| *c == ' ') {
                    i += 1;
                }
                let mut delimiter = String::new();
                while let Some(&c) = chars.get(i) {
                    if c == ' ' || c == '\n' || c == ';' || c == '|' || c == '&' || c == ')' {
                        break;
                    }
                    if c != '\'' && c != '"' && c != '\\' {
                        delimiter.push(c);
                    }
                    i += 1;
                }
                i -= 1;
                pending_heredocs.push((words.len(), delimiter, strip_tabs));
                words.push("<<\0".to_owned());
            }
            _ => {
                in_word = true;
                word.push(c);
            }
        }
        i += 1;
    }
    flush(&mut word, &mut in_word, &mut words);
    words
}

/// The body of a `"$(cat <<'EOF' ... EOF)"` word, the way models often pass multi-line text.
fn command_substitution_heredoc(word: &str) -> Option<String> {
    let inner = word.strip_prefix("$(")?.strip_suffix(')')?;
    let inner = inner.trim_start();
    let rest = inner.strip_prefix("cat")?.trim_start();
    let parsed = words(rest);
    parsed
        .iter()
        .find_map(|word| word.strip_prefix("<<\0").map(str::to_owned))
        .filter(|body| !body.is_empty())
}

fn value_text(word: &str) -> String {
    command_substitution_heredoc(word).unwrap_or_else(|| word.to_owned())
}

fn is_agentctl(word: &str) -> bool {
    let name = Path::new(word)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    name == "agentctl" || name.starts_with("agentctl-")
}

fn is_agent_teams(word: &str) -> bool {
    Path::new(word)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "agent-teams")
}

/// The flags of one simple command: `(flag values, positional words, here-document body)`.
fn arguments(rest: &[String]) -> (Vec<(String, String)>, Vec<String>, Option<String>) {
    let mut flags = Vec::new();
    let mut positional = Vec::new();
    let mut heredoc = None;
    let mut index = 0;
    while index < rest.len() {
        let word = &rest[index];
        if let Some(body) = word.strip_prefix("<<\0") {
            heredoc = Some(body.to_owned());
        } else if let Some(flag) = word.strip_prefix("--") {
            if let Some((name, value)) = flag.split_once('=') {
                flags.push((name.to_owned(), value.to_owned()));
            } else if let Some(value) = rest
                .get(index + 1)
                .filter(|next| !next.starts_with("--") && !next.starts_with("<<\0"))
            {
                flags.push((flag.to_owned(), value.clone()));
                index += 1;
            } else {
                flags.push((flag.to_owned(), String::new()));
            }
        } else {
            positional.push(word.clone());
        }
        index += 1;
    }
    (flags, positional, heredoc)
}

fn flag<'a>(flags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .rev()
        .find(|(flag, _)| flag == name)
        .map(|(_, value)| value.as_str())
}

/// Fill `text` from a here-document when `--file` reads standard input.
fn resolve(mut call: ReplyCall, heredoc: Option<String>) -> ReplyCall {
    if matches!(call.file.as_deref(), Some("-" | "/dev/stdin")) {
        call.file = None;
        if call.text.is_none() {
            call.text = heredoc;
        }
    }
    call
}

/// Recognise a send in a `Bash` command line. Only the first matching simple command counts.
#[must_use]
pub fn detect(command: &str) -> Option<ReplyCall> {
    let words = words(command);
    let commands =
        words.split(|word| matches!(word.as_str(), ";" | "&&" | "||" | "|" | "\n" | "&"));
    for simple in commands {
        // Skip leading environment assignments (`NAME=value`).
        let start = simple
            .iter()
            .position(|word| {
                !word.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
            })
            .unwrap_or(simple.len());
        let simple = &simple[start..];
        let Some(program) = simple.first() else {
            continue;
        };
        // Global options such as `--registry DIR` may come before the subcommand.
        let (flags, positional, heredoc) = arguments(&simple[1..]);
        let sub: Vec<&str> = positional.iter().map(String::as_str).collect();
        let text = flag(&flags, "text").map(value_text);
        let file = flag(&flags, "file").map(str::to_owned);
        if is_agentctl(program) {
            match sub.as_slice() {
                ["chat", "reply", ..] => {
                    let to = [flag(&flags, "request"), flag(&flags, "reply-id")]
                        .iter()
                        .flatten()
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" #");
                    return Some(resolve(
                        ReplyCall {
                            channel: "chat".to_owned(),
                            to,
                            text,
                            file,
                        },
                        heredoc,
                    ));
                }
                ["send", name, rest @ ..] => {
                    return Some(resolve(
                        ReplyCall {
                            channel: "agent".to_owned(),
                            to: (*name).to_owned(),
                            text: rest.first().map(|text| value_text(text)).or(text),
                            file,
                        },
                        heredoc,
                    ));
                }
                _ => {}
            }
        } else if is_agent_teams(program) {
            if let ["reply" | "send", to, ..] = sub.as_slice() {
                return Some(resolve(
                    ReplyCall {
                        channel: "team".to_owned(),
                        to: (*to).to_owned(),
                        text,
                        file,
                    },
                    heredoc,
                ));
            }
        }
    }
    None
}

/// The text of a file a send named, read after the call succeeded: at most [`REPLY_BYTES`],
/// UTF-8 only, relative paths taken from `cwd`. `None` when it cannot be read.
#[must_use]
pub fn read_file(path: &str, cwd: &Path) -> Option<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    if path.contains('$') || path.contains('`') {
        return None;
    }
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => Path::new(&std::env::var_os("HOME")?).join(rest),
        None => cwd.join(path),
    };
    // Never block the wrapper on a FIFO or device the command line happened to name.
    if !std::fs::metadata(&expanded).ok()?.is_file() {
        return None;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(expanded)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(REPLY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let truncated = bytes.len() > REPLY_BYTES;
    bytes.truncate(REPLY_BYTES);
    let mut text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if truncated => {
            let valid = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).ok()?
        }
        Err(_) => return None,
    };
    if truncated {
        text.push('…');
    }
    Some(text.trim_end_matches('\n').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(channel: &str, to: &str, text: Option<&str>, file: Option<&str>) -> ReplyCall {
        ReplyCall {
            channel: channel.to_owned(),
            to: to.to_owned(),
            text: text.map(str::to_owned),
            file: file.map(str::to_owned),
        }
    }

    #[test]
    fn recognises_chat_reply_with_a_file() {
        assert_eq!(
            detect("/home/u/bin/agentctl-f964db47 chat reply --bridge-state /s --request r-12 --reply-id 001 --file /tmp/reply.md"),
            Some(call("chat", "r-12 #001", None, Some("/tmp/reply.md")))
        );
    }

    #[test]
    fn recognises_team_replies_after_assignments_and_other_commands() {
        assert_eq!(
            detect("NOW=$(date -Is); AGENT_TEAMS_AGENTCTL=/x/agentctl ~/.local/bin/agent-teams reply tm-1 --text \"ACK-A $NOW (host)\""),
            Some(call("team", "tm-1", Some("ACK-A $NOW (host)"), None))
        );
        assert_eq!(
            detect("cd /w && agent-teams send peer --file=note.md"),
            Some(call("team", "peer", None, Some("note.md")))
        );
    }

    #[test]
    fn recognises_agentctl_send_with_a_positional_prompt() {
        assert_eq!(
            detect("agentctl --registry /r send worker-1 'please rebase' --urgent"),
            Some(call("agent", "worker-1", Some("please rebase"), None))
        );
    }

    #[test]
    fn here_documents_supply_the_text() {
        let command = "agentctl chat reply --request k --reply-id 002 --file - <<'EOF'\n## Done\n\n- one\n- two\nEOF\necho sent";
        assert_eq!(
            detect(command),
            Some(call(
                "chat",
                "k #002",
                Some("## Done\n\n- one\n- two"),
                None
            ))
        );
        let command =
            "agent-teams reply tm-2 --text \"$(cat <<'EOF'\nLine one\nLine \"two\"\nEOF\n)\"";
        assert_eq!(
            detect(command),
            Some(call("team", "tm-2", Some("Line one\nLine \"two\""), None))
        );
    }

    #[test]
    fn other_commands_are_not_sends() {
        assert_eq!(detect("git push origin main"), None);
        assert_eq!(detect("agentctl status coordinator"), None);
        assert_eq!(detect("echo agentctl send x y"), None);
        assert_eq!(detect("agentctl chat sent --request k"), None);
    }

    #[test]
    fn read_file_is_bounded_and_relative_to_cwd() {
        let dir = std::env::temp_dir().join(format!("agentctl-reply-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("r.md"), "hello\n\n").unwrap();
        assert_eq!(read_file("r.md", &dir).as_deref(), Some("hello"));
        std::fs::write(dir.join("big.md"), "é".repeat(REPLY_BYTES)).unwrap();
        let big = read_file("big.md", &dir).unwrap();
        assert!(big.len() <= REPLY_BYTES + 3 && big.ends_with('…'));
        assert_eq!(read_file("$HOME/x", &dir), None);
        assert_eq!(read_file("missing.md", &dir), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
