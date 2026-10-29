//! Reading and rewriting the unit text `systemctl cat` prints.
//!
//! The text is the definition `services refresh` backs up and replays, so parsing keeps every
//! line: sections and their `Key=Value` assignments in order, with the header comments that name
//! the files they came from. Values stay in unit-file form (quotes and `%` specifiers intact)
//! until a caller asks for words.

use std::fmt::Write as _;

/// One `Key=Value` line, in file order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assignment {
    /// The section the line sits in, such as `Unit` or `Service`.
    pub section: String,
    /// The key, such as `ExecStart`.
    pub key: String,
    /// The value exactly as written after `=`, continuation lines joined.
    pub value: String,
}

/// The parsed output of `systemctl cat UNIT`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnitText {
    /// The files the text came from, from its `# /path` header lines, in order.
    pub files: Vec<String>,
    /// Every assignment of every file, in order.
    pub assignments: Vec<Assignment>,
}

impl UnitText {
    /// Parse `systemctl cat` output. Comment lines are dropped except the `# /path` headers.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut parsed = Self::default();
        let mut section = String::new();
        let mut pending: Option<String> = None;
        for raw in text.lines() {
            let line = match pending.take() {
                Some(mut joined) => {
                    joined.push(' ');
                    joined.push_str(raw.trim_start());
                    joined
                }
                None => raw.to_owned(),
            };
            // A trailing backslash continues the line, as in any unit file.
            if let Some(stripped) = line.strip_suffix('\\') {
                if !line.trim_start().starts_with(['#', ';']) {
                    pending = Some(stripped.to_owned());
                    continue;
                }
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(path) = trimmed.strip_prefix("# /") {
                parsed.files.push(format!("/{}", path.trim_end()));
                continue;
            }
            if trimmed.starts_with(['#', ';']) {
                continue;
            }
            if let Some(name) = trimmed
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                section = name.to_owned();
                continue;
            }
            if let Some((key, value)) = trimmed.split_once('=') {
                parsed.assignments.push(Assignment {
                    section: section.clone(),
                    key: key.trim().to_owned(),
                    value: value.trim().to_owned(),
                });
            }
        }
        parsed
    }

    /// The values of `key` in `section` that are in effect: an empty assignment resets the list,
    /// as systemd treats `ExecStart=` and `Environment=`.
    #[must_use]
    pub fn effective(&self, section: &str, key: &str) -> Vec<&str> {
        let mut values = Vec::new();
        for assignment in &self.assignments {
            if assignment.section == section && assignment.key == key {
                if assignment.value.is_empty() {
                    values.clear();
                } else {
                    values.push(assignment.value.as_str());
                }
            }
        }
        values
    }

    /// The last value of `key` in `section`.
    #[must_use]
    pub fn last(&self, section: &str, key: &str) -> Option<&str> {
        self.effective(section, key).last().copied()
    }

    /// The `Environment=` variables in effect, in order, a later assignment of a name replacing
    /// an earlier one. Values are unquoted; `%` specifiers are left as written.
    pub fn environment(&self) -> Result<Vec<(String, String)>, String> {
        let mut variables: Vec<(String, String)> = Vec::new();
        for value in self.effective("Service", "Environment") {
            for word in split_words(value)? {
                let (name, value) = word
                    .split_once('=')
                    .ok_or_else(|| format!("Environment word {word:?} has no ="))?;
                variables.retain(|(existing, _)| existing != name);
                variables.push((name.to_owned(), value.to_owned()));
            }
        }
        Ok(variables)
    }
}

/// The command of an `ExecStart=` value: its prefix characters and its words.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecCommand {
    /// The special prefixes (`-`, `@`, `:`, `+`, `!`) written before the program.
    pub prefix: String,
    /// The program and its arguments, unquoted; `%` specifiers are left as written.
    pub argv: Vec<String>,
}

impl ExecCommand {
    /// Parse an `ExecStart=` value.
    pub fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim_start();
        let prefix: String = trimmed
            .chars()
            .take_while(|character| "-@:+!".contains(*character))
            .collect();
        let argv = split_words(&trimmed[prefix.len()..])?;
        if argv.is_empty() {
            return Err("ExecStart= names no program".to_owned());
        }
        Ok(Self { prefix, argv })
    }

    /// The value for a unit file: the prefix, then every word double-quoted.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = self.prefix.clone();
        for (index, word) in self.argv.iter().enumerate() {
            if index > 0 {
                text.push(' ');
            }
            text.push_str(&quote_word(word));
        }
        text
    }
}

/// Split a unit-file value into words: white space separates them, and double or single quotes
/// group them, with C-style backslash escapes.
pub fn split_words(value: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut characters = value.chars().peekable();
    loop {
        while characters.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = characters.peek() else {
            return Ok(words);
        };
        let mut word = String::new();
        if first == '"' || first == '\'' {
            characters.next();
            loop {
                match characters.next() {
                    None => return Err(format!("unterminated quote in {value:?}")),
                    Some(c) if c == first => break,
                    Some('\\') => word.push(unescape(&mut characters, value)?),
                    Some(c) => word.push(c),
                }
            }
            if characters.peek().is_some_and(|c| !c.is_whitespace()) {
                return Err(format!("quoted word is followed by text in {value:?}"));
            }
        } else {
            while let Some(c) = characters.next_if(|c| !c.is_whitespace()) {
                if c == '\\' {
                    word.push(unescape(&mut characters, value)?);
                } else {
                    word.push(c);
                }
            }
        }
        words.push(word);
    }
}

fn unescape(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    value: &str,
) -> Result<char, String> {
    match characters.next() {
        Some('n') => Ok('\n'),
        Some('t') => Ok('\t'),
        Some('r') => Ok('\r'),
        Some('s') => Ok(' '),
        Some(c @ ('\\' | '"' | '\'' | ' ')) => Ok(c),
        Some(other) => Err(format!("unsupported escape \\{other} in {value:?}")),
        None => Err(format!("trailing backslash in {value:?}")),
    }
}

/// One word double-quoted for a unit file, so [`split_words`] reads it back unchanged.
#[must_use]
pub fn quote_word(word: &str) -> String {
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('"');
    for character in word.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Undo the `%%` escaping systemd applies when it writes a transient unit file. A lone `%`
/// would be a specifier, which a transient definition never holds; it is refused.
pub fn unescape_transient(value: &str) -> Result<String, String> {
    let mut text = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '%' && characters.next() != Some('%') {
            return Err(format!("specifier in transient unit text {value:?}"));
        }
        text.push(character);
    }
    Ok(text)
}

/// One word of a `sh -c` script, with where it sits in the script.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellWord {
    /// The word with quotes removed.
    pub text: String,
    /// Byte offset of the word's first character in the script.
    pub start: usize,
    /// Byte offset just past the word.
    pub end: usize,
    /// Whether the word is an operator (`;`, `&&`, `|`, ...) or a redirection.
    pub operator: bool,
}

/// Split a POSIX shell script into words, closely enough to find the first simple command:
/// quotes and backslashes are honoured; operators and redirections are words of their own.
/// Expansions (`$`, backquotes, globs) are refused: a script that computes its command cannot be
/// rewritten safely.
pub fn shell_words(script: &str) -> Result<Vec<ShellWord>, String> {
    let bytes = script.as_bytes();
    let mut words = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte == b'#' {
            break;
        }
        let start = index;
        // Operators, and redirections with an optional file-descriptor digit.
        let redirection_start = if byte.is_ascii_digit()
            && bytes
                .get(index + 1)
                .is_some_and(|next| *next == b'>' || *next == b'<')
        {
            index + 1
        } else {
            index
        };
        if matches!(bytes[redirection_start], b'>' | b'<' | b';' | b'&' | b'|') {
            let mut end = redirection_start + 1;
            while end < bytes.len() && matches!(bytes[end], b'>' | b'<' | b'&' | b'|') {
                end += 1;
            }
            let redirect = matches!(bytes[redirection_start], b'>' | b'<');
            if redirect {
                // `2>&1` is complete; otherwise the file name follows, joined or after spaces.
                while end < bytes.len() && bytes[end].is_ascii_whitespace() {
                    end += 1;
                }
                while end < bytes.len()
                    && !bytes[end].is_ascii_whitespace()
                    && !matches!(bytes[end], b';' | b'|')
                {
                    end += 1;
                }
            }
            words.push(ShellWord {
                text: script[start..end].trim().to_owned(),
                start,
                end,
                operator: true,
            });
            index = end;
            continue;
        }
        let mut text = String::new();
        while index < bytes.len() {
            let current = bytes[index];
            if current.is_ascii_whitespace() || matches!(current, b';' | b'&' | b'|' | b'>' | b'<')
            {
                break;
            }
            match current {
                b'\'' => {
                    let close = script[index + 1..]
                        .find('\'')
                        .ok_or_else(|| "unterminated single quote in the script".to_owned())?;
                    text.push_str(&script[index + 1..index + 1 + close]);
                    index += close + 2;
                }
                b'"' => {
                    index += 1;
                    loop {
                        match bytes.get(index) {
                            None => {
                                return Err("unterminated double quote in the script".to_owned())
                            }
                            Some(b'"') => {
                                index += 1;
                                break;
                            }
                            Some(b'\\') if index + 1 < bytes.len() => {
                                let next = script[index + 1..].chars().next().unwrap_or('\\');
                                if !matches!(next, '"' | '\\' | '$' | '`') {
                                    text.push('\\');
                                }
                                text.push(next);
                                index += 1 + next.len_utf8();
                            }
                            Some(b'$' | b'`') => {
                                return Err("the script expands a variable or command".to_owned())
                            }
                            Some(_) => {
                                let character = script[index..].chars().next().unwrap_or(' ');
                                text.push(character);
                                index += character.len_utf8();
                            }
                        }
                    }
                }
                b'\\' => {
                    let next = script[index + 1..]
                        .chars()
                        .next()
                        .ok_or_else(|| "trailing backslash in the script".to_owned())?;
                    text.push(next);
                    index += 1 + next.len_utf8();
                }
                b'$' | b'`' | b'*' | b'?' | b'[' | b'~' | b'(' | b')' | b'{' | b'}' => {
                    return Err(format!(
                        "the script uses shell syntax ({}) that cannot be rewritten safely",
                        current as char
                    ));
                }
                _ => {
                    let character = script[index..].chars().next().unwrap_or(' ');
                    text.push(character);
                    index += character.len_utf8();
                }
            }
        }
        words.push(ShellWord {
            text,
            start,
            end: index,
            operator: false,
        });
    }
    Ok(words)
}

/// Quote a word for a POSIX shell when it needs quoting.
#[must_use]
pub fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-+=:,@%".contains(&byte))
    {
        return word.to_owned();
    }
    let mut quoted = String::from("'");
    for character in word.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

/// A shell command line from words, each quoted when it needs it.
#[must_use]
pub fn shell_line(words: &[String]) -> String {
    let mut line = String::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            line.push(' ');
        }
        let _ = write!(line, "{}", shell_quote(word));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSIENT: &str = r#"# /run/user/1000/systemd/transient/bridge.service
# This is a transient unit file, created programmatically via the systemd API. Do not edit.
[Unit]
Description=chat bridge

[Service]
Restart=on-failure
WorkingDirectory=/work/team
Environment="AGENTCTL_HOME=/state/home-old" "PATH=/usr/bin:/bin" "HOME=/home/u"
ExecStart=
ExecStart="/bin/sh" "-c" "exec /state/bin/agentctl-old --registry /work/team/.agentctl chat run --bridge-state /state/bridge 2>>/state/logs/bridge.log"
"#;

    #[test]
    fn parses_a_transient_unit() {
        let unit = UnitText::parse(TRANSIENT);
        assert_eq!(
            unit.files,
            ["/run/user/1000/systemd/transient/bridge.service"]
        );
        assert_eq!(unit.last("Unit", "Description"), Some("chat bridge"));
        let exec = unit.effective("Service", "ExecStart");
        assert_eq!(exec.len(), 1);
        let command = ExecCommand::parse(exec[0]).unwrap();
        assert_eq!(command.argv[0], "/bin/sh");
        assert_eq!(command.argv[1], "-c");
        assert!(command.argv[2].starts_with("exec /state/bin/agentctl-old "));
        assert_eq!(
            unit.environment().unwrap(),
            [
                ("AGENTCTL_HOME".to_owned(), "/state/home-old".to_owned()),
                ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
                ("HOME".to_owned(), "/home/u".to_owned()),
            ]
        );
    }

    #[test]
    fn exec_command_round_trips_through_render() {
        let command =
            ExecCommand::parse(r#"-"/bin/x" "a b" plain "q\"uote" "back\\slash""#).expect("parse");
        assert_eq!(command.prefix, "-");
        assert_eq!(
            command.argv,
            ["/bin/x", "a b", "plain", "q\"uote", "back\\slash"]
        );
        assert_eq!(ExecCommand::parse(&command.render()).unwrap(), command);
    }

    #[test]
    fn later_environment_assignments_win_and_empty_resets() {
        let unit = UnitText::parse(
            "[Service]\nEnvironment=A=1 B=2\nEnvironment=\nEnvironment=\"A=3\"\nEnvironment=C=4 A=5\n",
        );
        assert_eq!(
            unit.environment().unwrap(),
            [
                ("C".to_owned(), "4".to_owned()),
                ("A".to_owned(), "5".to_owned())
            ]
        );
    }

    #[test]
    fn continuation_lines_join() {
        let unit = UnitText::parse("[Service]\nExecStart=/bin/agentctl \\\n  watch-agents\n");
        assert_eq!(
            unit.last("Service", "ExecStart"),
            Some("/bin/agentctl  watch-agents")
        );
    }

    #[test]
    fn transient_percent_escapes_are_undone_and_specifiers_refused() {
        assert_eq!(unescape_transient("50%% done").unwrap(), "50% done");
        assert!(unescape_transient("%h/x").is_err());
    }

    #[test]
    fn shell_words_find_the_command_and_its_redirection() {
        let script = "exec /s/bin/agentctl --registry '/w/a b' chat run 2>>/s/log";
        let words = shell_words(script).unwrap();
        let texts: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "exec",
                "/s/bin/agentctl",
                "--registry",
                "/w/a b",
                "chat",
                "run",
                "2>>/s/log"
            ]
        );
        assert!(words[6].operator);
        assert_eq!(&script[words[1].start..words[1].end], "/s/bin/agentctl");
        assert!(shell_words("exec $AGENTCTL watch-agents").is_err());
        let spaced = shell_words("a > /log; b").unwrap();
        assert_eq!(spaced[1].text, "> /log");
        assert_eq!(spaced[2].text, ";");
    }

    #[test]
    fn shell_quote_leaves_plain_words_alone() {
        assert_eq!(shell_quote("/a/b-c_d.e"), "/a/b-c_d.e");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
