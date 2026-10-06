//! gh's own command aliases, read from gh's configuration file, so the content guard can see the
//! body an alias expansion sends.
//!
//! gh expands an ordinary alias inside its own process: `gh upload 7` with the alias
//! `upload: api -X POST repos/o/r/issues/$1/comments --input -` sends stdin as the request body
//! without any second gh-paced invocation seeing `--input -`. gh-paced therefore expands the
//! alias the way gh does and inspects the body sources of the expansion as well.
//!
//! Only ordinary aliases are expanded. A shell alias (`!...`) runs `sh -c`; a `gh` that the shell
//! command runs is a new gh-paced invocation when gh-paced is installed in front of `gh` on
//! `PATH`, and is inspected there.

use std::path::PathBuf;

/// Deepest chain of aliases followed (an alias whose expansion starts with another alias).
pub const MAX_ALIAS_DEPTH: usize = 5;

/// gh's configuration file: `$GH_CONFIG_DIR/config.yml`, else
/// `$XDG_CONFIG_HOME/gh/config.yml`, else `~/.config/gh/config.yml`, as gh looks it up.
pub fn config_file(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(d) = set("GH_CONFIG_DIR") {
        return Some(PathBuf::from(d).join("config.yml"));
    }
    if let Some(d) = set("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d).join("gh").join("config.yml"));
    }
    set("HOME").map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("gh")
            .join("config.yml")
    })
}

/// The aliases in gh's configuration file, or none when it cannot be read.
pub fn load(env: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    config_file(env)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_aliases(&t))
        .unwrap_or_default()
}

/// The `aliases:` mapping of a gh `config.yml`, as `(name, expansion)` pairs.
///
/// This reads the block form gh writes (`aliases:` at the left margin, then one indented
/// `name: expansion` per line, each value plain, single-quoted or double-quoted), plus block
/// scalars (`|`, `>`) and indented continuation lines. A flow mapping (`aliases: {co: ...}`),
/// which gh does not write, yields no aliases.
pub fn parse_aliases(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut lines = text.lines().peekable();
    // Find `aliases:` at the left margin.
    loop {
        match lines.next() {
            None => return out,
            Some(l) => {
                let l = strip_comment(l).trim_end();
                if l == "aliases:" {
                    break;
                }
            }
        }
    }
    let mut entry_indent = None;
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            break;
        }
        let entry = *entry_indent.get_or_insert(indent);
        if indent > entry {
            // A continuation of the previous plain value.
            if let Some((_, v)) = out.last_mut() {
                v.push(' ');
                v.push_str(strip_comment(trimmed).trim());
            }
            continue;
        }
        let Some((key, rest)) = split_key(trimmed) else {
            continue;
        };
        let rest = rest.trim();
        let value = if rest.starts_with('|') || rest.starts_with('>') {
            let mut parts = Vec::new();
            while let Some(next) = lines.peek() {
                let t = next.trim_start();
                if !t.is_empty() && next.len() - t.len() <= entry {
                    break;
                }
                parts.push(t.trim_end().to_string());
                lines.next();
            }
            parts.join("\n")
        } else {
            scalar(rest)
        };
        out.push((key, value));
    }
    out
}

/// The value of a top-level `key: value` line of a gh `config.yml` (such as `editor`), or `None`
/// when the key is absent. A key that is only indented, inside another mapping, does not count.
pub fn top_level_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter(|l| !l.starts_with([' ', '\t']))
        .filter_map(|l| split_key(strip_comment(l).trim_end()))
        .find(|(k, _)| k == key)
        .map(|(_, rest)| scalar(rest.trim()))
}

/// Drop a ` #` comment from a plain line.
fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    match line.find(" #") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Split `key: value` (the key plain or quoted).
fn split_key(line: &str) -> Option<(String, &str)> {
    if line.starts_with('\'') || line.starts_with('"') {
        let (key, used) = quoted(line)?;
        let rest = line[used..].trim_start().strip_prefix(':')?;
        return Some((key, rest));
    }
    let i = line
        .find(": ")
        .or_else(|| line.strip_suffix(':').map(str::len))?;
    let rest = line.get(i + 1..).unwrap_or("");
    Some((line[..i].trim().to_string(), rest))
}

/// A YAML scalar value: quoted, or plain with any trailing comment removed.
fn scalar(text: &str) -> String {
    if text.starts_with('\'') || text.starts_with('"') {
        if let Some((v, _)) = quoted(text) {
            return v;
        }
    }
    strip_comment(text).trim().to_string()
}

/// A single- or double-quoted YAML scalar at the start of `text`: its value and the bytes used.
fn quoted(text: &str) -> Option<(String, usize)> {
    let mut chars = text.char_indices();
    let (_, q) = chars.next()?;
    let mut out = String::new();
    while let Some((i, ch)) = chars.next() {
        match (q, ch) {
            ('\'', '\'') => {
                if text[i + 1..].starts_with('\'') {
                    out.push('\'');
                    chars.next();
                } else {
                    return Some((out, i + 1));
                }
            }
            ('"', '"') => return Some((out, i + 1)),
            ('"', '\\') => {
                let (_, e) = chars.next()?;
                let hex = |n: usize, chars: &mut std::str::CharIndices| -> Option<char> {
                    let s: String = (0..n)
                        .filter_map(|_| chars.next().map(|(_, c)| c))
                        .collect();
                    char::from_u32(u32::from_str_radix(&s, 16).ok()?)
                };
                let c = match e {
                    '0' => '\0',
                    'a' => '\x07',
                    'b' => '\x08',
                    't' | '\t' => '\t',
                    'n' => '\n',
                    'v' => '\x0b',
                    'f' => '\x0c',
                    'r' => '\r',
                    'e' => '\x1b',
                    'N' => '\u{85}',
                    '_' => '\u{a0}',
                    'L' => '\u{2028}',
                    'P' => '\u{2029}',
                    'x' => hex(2, &mut chars)?,
                    'u' => hex(4, &mut chars)?,
                    'U' => hex(8, &mut chars)?,
                    other => other,
                };
                out.push(c);
            }
            _ => out.push(ch),
        }
    }
    None
}

/// Split `s` into words the way gh splits an alias expansion (github.com/google/shlex): blanks
/// separate words; `'...'` is literal; inside `"..."` a backslash takes the next character
/// literally; outside quotes a backslash does the same; `#` at the start of a word begins a
/// comment to the end of the line. `None` when a quote or escape is left open (gh refuses the
/// alias then).
pub fn split(s: &str) -> Option<Vec<String>> {
    #[derive(PartialEq)]
    enum St {
        Start,
        Word,
        Comment,
    }
    let mut out = Vec::new();
    let mut word = String::new();
    let mut st = St::Start;
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        match st {
            St::Comment => {
                if ch == '\n' {
                    st = St::Start;
                }
                continue;
            }
            St::Start if ch == '#' => {
                st = St::Comment;
                continue;
            }
            _ => {}
        }
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                if st == St::Word {
                    out.push(std::mem::take(&mut word));
                }
                st = St::Start;
            }
            '\\' => {
                word.push(chars.next()?);
                st = St::Word;
            }
            '\'' => {
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
                st = St::Word;
            }
            '"' => {
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => word.push(chars.next()?),
                        c => word.push(c),
                    }
                }
                st = St::Word;
            }
            c => {
                word.push(c);
                st = St::Word;
            }
        }
    }
    if st == St::Word {
        out.push(word);
    }
    Some(out)
}

/// Expand one ordinary alias as gh does. gh walks the arguments in order: while the expansion
/// still contains a `$`, argument N replaces every `$N`; once no `$` is left, each remaining
/// argument is appended. So `issue comment $1` given `7 extra` becomes
/// `issue comment 7 extra`. The text is then split into words. `None` when gh would refuse it
/// (a `$` and a digit left over, or an open quote).
pub fn expand(expansion: &str, args: &[String]) -> Option<Vec<String>> {
    let mut text = expansion.to_string();
    let mut extra = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if text.contains('$') {
            text = text.replace(&format!("${}", i + 1), a);
        } else {
            extra.push(a.clone());
        }
    }
    let bytes = text.as_bytes();
    if bytes
        .windows(2)
        .any(|w| w[0] == b'$' && w[1].is_ascii_digit())
    {
        return None;
    }
    let mut words = split(&text)?;
    words.extend(extra);
    Some(words)
}

/// What a command line resolves to through gh's aliases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The command line does not start with an alias (or gh would refuse the expansion).
    NotAlias,
    /// It reaches a shell alias, which gh runs with `sh -c`.
    Shell,
    /// The command line gh runs after expanding the alias (and any aliases it starts with).
    Expanded(Vec<String>),
}

/// Resolve `args` (gh's arguments, without global flags) through `aliases`. The longest alias
/// name whose words begin `args` is used, since gh accepts names of several words
/// (`issue publish`). Aliases are followed up to [`MAX_ALIAS_DEPTH`] deep.
pub fn resolve(args: &[String], aliases: &[(String, String)]) -> Resolution {
    let mut current = args.to_vec();
    let mut expanded = false;
    for _ in 0..MAX_ALIAS_DEPTH {
        let best = aliases
            .iter()
            .filter_map(|(name, value)| {
                let words: Vec<&str> = name.split_whitespace().collect();
                let matches = !words.is_empty()
                    && words.len() <= current.len()
                    && words.iter().zip(&current).all(|(w, a)| w == a);
                matches.then_some((words.len(), value))
            })
            .max_by_key(|(n, _)| *n);
        let Some((n, value)) = best else {
            break;
        };
        if value.starts_with('!') {
            return Resolution::Shell;
        }
        match expand(value, &current[n..]) {
            Some(next) => {
                current = next;
                expanded = true;
            }
            None => return Resolution::NotAlias,
        }
    }
    if expanded {
        Resolution::Expanded(current)
    } else {
        Resolution::NotAlias
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    /// The aliases block exactly as gh 2.97 wrote it for these `gh alias set` calls.
    const CONFIG: &str = "\
# Aliases allow you to create nicknames for gh commands
aliases:
    co: pr checkout
    upload: api -X POST repos/o/r/issues/$1/comments --input -
    sh1: '!echo hi | cat'
    q1: 'issue comment $1 --body ''it''''s: a #test'''
    q2: api -F \"body=@-\" \"repos/o/r/issues/$1/comments\"
    nest: upload
    issue publish: api -X POST x --input -
    \"dq\": \"api --input \\x2d x\"
# The path to a unix socket
http_unix_socket:
";

    #[test]
    fn reads_the_aliases_gh_writes() {
        let a = parse_aliases(CONFIG);
        let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("co"), Some("pr checkout"));
        assert_eq!(
            get("upload"),
            Some("api -X POST repos/o/r/issues/$1/comments --input -")
        );
        assert_eq!(get("sh1"), Some("!echo hi | cat"));
        assert_eq!(get("q1"), Some("issue comment $1 --body 'it''s: a #test'"));
        assert_eq!(
            get("q2"),
            Some("api -F \"body=@-\" \"repos/o/r/issues/$1/comments\"")
        );
        assert_eq!(get("issue publish"), Some("api -X POST x --input -"));
        assert_eq!(get("dq"), Some("api --input - x"));
        assert_eq!(a.len(), 8, "{a:?}");
        assert!(parse_aliases("version: 1\naliases: {}\n").is_empty());
        assert!(parse_aliases("").is_empty());
    }

    #[test]
    fn splits_like_gh() {
        assert_eq!(
            split(r#"a 'b c' "d \" e" f\ g h#i #comment"#).unwrap(),
            vec!["a", "b c", "d \" e", "f g", "h#i"]
        );
        assert_eq!(split(r#"x "" y"#).unwrap(), vec!["x", "", "y"]);
        assert_eq!(split("'open"), None);
        assert_eq!(split("trailing\\"), None);
    }

    #[test]
    fn expands_like_gh() {
        assert_eq!(
            expand("pr checkout", &argv("12 --force")).unwrap(),
            argv("pr checkout 12 --force")
        );
        // While a `$` is left, an argument goes only where its `$N` puts it; once none is
        // left, the rest are appended (gh checks for `$` before each argument).
        assert_eq!(
            expand("issue comment $1 --body-file -", &argv("7")).unwrap(),
            argv("issue comment 7 --body-file -")
        );
        assert_eq!(
            expand("issue comment $1 --body-file -", &argv("7 extra")).unwrap(),
            argv("issue comment 7 --body-file - extra")
        );
        assert_eq!(
            expand("api repos/$1/$2 --input -", &argv("o r --verbose")).unwrap(),
            argv("api repos/o/r --input - --verbose")
        );
        // A substituted argument is split into words with the rest of the text.
        assert_eq!(
            expand("issue comment $1", &["7 --body-file big.md".to_string()]).unwrap(),
            argv("issue comment 7 --body-file big.md")
        );
        assert_eq!(expand("issue comment $2", &argv("7")), None);
    }

    #[test]
    fn resolves_nested_compound_and_shell_aliases() {
        let a = parse_aliases(CONFIG);
        assert_eq!(
            resolve(&argv("upload 7"), &a),
            Resolution::Expanded(argv("api -X POST repos/o/r/issues/7/comments --input -"))
        );
        assert_eq!(
            resolve(&argv("nest 7"), &a),
            Resolution::Expanded(argv("api -X POST repos/o/r/issues/7/comments --input -"))
        );
        assert_eq!(
            resolve(&argv("issue publish --help"), &a),
            Resolution::Expanded(argv("api -X POST x --input - --help"))
        );
        assert_eq!(resolve(&argv("sh1"), &a), Resolution::Shell);
        assert_eq!(resolve(&argv("pr view 1"), &a), Resolution::NotAlias);
        // A loop stops at the depth limit.
        let looped = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "a".to_string()),
        ];
        assert!(matches!(
            resolve(&argv("a"), &looped),
            Resolution::Expanded(_)
        ));
    }

    #[test]
    fn finds_gh_config_like_gh() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            config_file(&env(&[("GH_CONFIG_DIR", "/c"), ("HOME", "/h")])),
            Some(PathBuf::from("/c/config.yml"))
        );
        assert_eq!(
            config_file(&env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")])),
            Some(PathBuf::from("/x/gh/config.yml"))
        );
        assert_eq!(
            config_file(&env(&[("HOME", "/h")])),
            Some(PathBuf::from("/h/.config/gh/config.yml"))
        );
    }
}
