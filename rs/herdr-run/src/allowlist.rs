//! Command admission and safe shell rendering.

use std::ffi::{CStr, CString};

use crate::config::Config;
use crate::error::{HerdrRunError, Result};

/// An admitted command together with the policy facts used to admit it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Admission {
    /// Full argv, including any allowed wrapper prefixes.
    pub argv: Vec<String>,
    /// Allowed wrapper chain preceding the program.
    pub prefix: Vec<String>,
    /// The allowlisted bare program name.
    pub program: String,
    /// First non-option program argument, when present.
    pub subcommand: Option<String>,
    rendered: String,
}

impl Admission {
    /// Render this admission as an exact, injection-safe shell word list.
    #[must_use]
    pub fn rendered(&self) -> &str {
        &self.rendered
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SplitState {
    Unquoted,
    SingleQuoted,
    DoubleQuoted,
    EscapedUnquoted,
    EscapedDouble,
}

fn split(command: &str) -> Result<Vec<String>> {
    // This small state machine intentionally does not use shell_words::split: that parser enables
    // shell comments, while Python's shlex.split(..., comments=False, posix=True) treats `#` as an
    // ordinary argument character. It also preserves Python shlex's narrower double-quote escape
    // rule: only backslash and double quote lose the backslash inside double quotes.
    let mut state = SplitState::Unquoted;
    let mut token = String::new();
    let mut token_started = false;
    let mut output = Vec::new();
    for character in command.chars() {
        match state {
            SplitState::Unquoted => match character {
                ' ' | '\t' | '\r' | '\n' => {
                    if token_started {
                        output.push(std::mem::take(&mut token));
                        token_started = false;
                    }
                }
                '\'' => {
                    token_started = true;
                    state = SplitState::SingleQuoted;
                }
                '"' => {
                    token_started = true;
                    state = SplitState::DoubleQuoted;
                }
                '\\' => {
                    token_started = true;
                    state = SplitState::EscapedUnquoted;
                }
                other => {
                    token_started = true;
                    token.push(other);
                }
            },
            SplitState::SingleQuoted => {
                if character == '\'' {
                    state = SplitState::Unquoted;
                } else {
                    token.push(character);
                }
            }
            SplitState::DoubleQuoted => match character {
                '"' => state = SplitState::Unquoted,
                '\\' => state = SplitState::EscapedDouble,
                other => token.push(other),
            },
            SplitState::EscapedUnquoted => {
                token.push(character);
                state = SplitState::Unquoted;
            }
            SplitState::EscapedDouble => {
                if character != '\\' && character != '"' {
                    token.push('\\');
                }
                token.push(character);
                state = SplitState::DoubleQuoted;
            }
        }
    }
    match state {
        SplitState::SingleQuoted | SplitState::DoubleQuoted => {
            return Err(HerdrRunError::refused(
                "cannot parse command (unbalanced quoting?): No closing quotation",
            ));
        }
        SplitState::EscapedUnquoted | SplitState::EscapedDouble => {
            return Err(HerdrRunError::refused(
                "cannot parse command (unbalanced quoting?): No escaped character",
            ));
        }
        SplitState::Unquoted => {}
    }
    if token_started {
        output.push(token);
    }
    if output.is_empty() {
        return Err(HerdrRunError::refused("empty command"));
    }
    Ok(output)
}

fn first_terminal_control(value: &str) -> Option<u32> {
    value
        .chars()
        .map(u32::from)
        .find(|codepoint| matches!(codepoint, 0x00..=0x1f | 0x7f..=0x9f))
}

fn control_refusal(value: &str, what: &str) -> Option<HerdrRunError> {
    first_terminal_control(value).map(|codepoint| {
        HerdrRunError::refused(format!(
            "{what} contains terminal control U+{codepoint:04X}; control characters cannot be injected into a shared pane"
        ))
    })
}

/// Admit `command` under `config`, or return a typed refusal before any pane interaction.
pub fn admit(command: &str, config: &Config) -> Result<Admission> {
    if let Some(error) = control_refusal(command, "command") {
        return Err(error);
    }
    let argv = split(command)?;
    let mut rest = argv.as_slice();
    let mut prefix = Vec::new();
    while let Some(first) = rest.first() {
        if !config.prefixes.contains(first) {
            break;
        }
        if prefix.contains(first) {
            return Err(HerdrRunError::refused(format!(
                "prefix {} repeated; each wrapper may appear at most once",
                python_repr(first)
            )));
        }
        prefix.push(first.clone());
        rest = &rest[1..];
    }
    if rest.is_empty() {
        return Err(HerdrRunError::refused(format!(
            "command is only wrapper prefixes ({}) with no program. Allowed programs: {}",
            prefix.join(" "),
            sorted_join(&config.allow)
        )));
    }
    let program = rest[0].clone();
    if program.contains('/') {
        return Err(HerdrRunError::refused(format!(
            "program must be a bare command name resolved from PATH, not a path: {}",
            python_repr(&program)
        )));
    }
    if !config.allows_any_program() && !config.allow.contains(&program) {
        let hint = if config.prefixes.contains(&program) {
            format!(
                " ({} is a wrapper prefix, not a program)",
                python_repr(&program)
            )
        } else {
            String::new()
        };
        return Err(HerdrRunError::refused(format!(
            "program {} is not allowlisted{hint}. Allowed: {}",
            python_repr(&program),
            sorted_join(&config.allow)
        )));
    }

    let args = &rest[1..];
    for token in args {
        let base = token
            .split_once('=')
            .map_or(token.as_str(), |(name, _)| name);
        if config.deny_anywhere.iter().any(|denied| denied == base) {
            return Err(HerdrRunError::refused(format!(
                "option {} is denied: it names a program for {program} to execute",
                python_repr(token)
            )));
        }
    }

    let deny_global = config
        .deny_global
        .get(&program)
        .map_or(&[][..], Vec::as_slice);
    let value_options = config
        .value_options
        .get(&program)
        .map_or(&[][..], Vec::as_slice);

    // Cargo accepts its "global" flags on either side of the subcommand. Check every token first,
    // including clap's attached/clustered short forms (`-Zfoo`, `-Z=foo`, `-qZfoo`).
    if program == "cargo" {
        let cargo_denied = deny_global
            .iter()
            .map(String::as_str)
            .chain(["--config", "-Z"]);
        for token in args {
            if let Some(denied) = cargo_denied
                .clone()
                .find(|denied| matches_denied_option(token, denied))
            {
                return Err(HerdrRunError::refused(format!(
                    "option {} is denied everywhere for cargo: it can make cargo execute arbitrary code",
                    python_repr(denied)
                )));
            }
        }
    }

    let mut subcommand = None;
    let mut expect_value = false;
    for token in args {
        if expect_value {
            expect_value = false;
            continue;
        }
        if !token.starts_with('-') {
            subcommand = Some(token.clone());
            break;
        }
        let (base, has_inline_value) = token
            .split_once('=')
            .map_or((token.as_str(), false), |(name, _)| (name, true));
        if let Some(denied) = deny_global
            .iter()
            .find(|denied| matches_denied_option(token, denied))
        {
            return Err(HerdrRunError::refused(format!(
                "global option {} is denied for {program}: it can make {program} execute arbitrary code",
                python_repr(denied)
            )));
        }
        if !has_inline_value && value_options.iter().any(|option| option == base) {
            expect_value = true;
        }
    }
    let allowed_subcommands = config.allow_subcommand.get(&program);
    if program == "cargo" && allowed_subcommands.is_none() {
        return Err(HerdrRunError::refused(
            "cargo requires an explicit allow_subcommand entry; omitting its positive list would silently admit compilation-oriented and unknown subcommands",
        ));
    }
    if let Some(allowed) = allowed_subcommands {
        let Some(candidate) = subcommand.as_deref() else {
            return Err(HerdrRunError::refused(format!(
                "{program} requires a subcommand, and only these are allowed: {}",
                sorted_join(allowed)
            )));
        };
        if !allowed.iter().any(|item| item == candidate) {
            return Err(HerdrRunError::refused(format!(
                "subcommand '{program} {candidate}' is not allowlisted. Allowed: {}. {program} compilation-oriented and unknown subcommands are deliberately excluded. Cargo must also be explicitly allowlisted because even dependency commands may execute ambiently configured helpers.",
                sorted_join(allowed)
            )));
        }
    }
    if let Some(candidate) = subcommand.as_deref() {
        if config
            .deny_subcommand
            .get(&program)
            .is_some_and(|denied| denied.iter().any(|item| item == candidate))
        {
            return Err(HerdrRunError::refused(format!(
                "subcommand '{program} {candidate}' is denied: it defines or runs arbitrary code"
            )));
        }
    }
    let rendered = render(&argv)?;
    Ok(Admission {
        argv,
        prefix,
        program,
        subcommand,
        rendered,
    })
}

fn matches_denied_option(token: &str, denied: &str) -> bool {
    if token == denied
        || token
            .strip_prefix(denied)
            .is_some_and(|rest| rest.starts_with('='))
    {
        return true;
    }
    let mut denied_chars = denied.chars();
    let Some('-') = denied_chars.next() else {
        return false;
    };
    let Some(short) = denied_chars.next() else {
        return false;
    };
    if denied_chars.next().is_some() || token.starts_with("--") || !token.starts_with('-') {
        return false;
    }
    token[1..].contains(short)
}

fn sorted_join(values: &[String]) -> String {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.join(", ")
}

fn python_repr(value: &str) -> String {
    // Policy diagnostics overwhelmingly contain command-line ASCII. Matching Python's preferred
    // quote choice and escapes here keeps those user-visible refusals byte-identical as well.
    let delimiter = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut output = String::from(delimiter);
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\x08' => output.push_str("\\x08"),
            '\x0c' => output.push_str("\\x0c"),
            control if control.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(output, "\\x{:02x}", u32::from(control));
            }
            quote if quote == delimiter => {
                output.push('\\');
                output.push(quote);
            }
            other => output.push(other),
        }
    }
    output.push(delimiter);
    output
}

/// Expand a token's leading `~` or `~user` without allowing a shell to interpret the result.
#[must_use]
pub fn expand_tilde(token: &str) -> String {
    if !token.starts_with('~') {
        return token.to_owned();
    }
    let slash = token.find('/').unwrap_or(token.len());
    let user = &token[1..slash];
    let suffix = &token[slash..];
    let home = if user.is_empty() {
        std::env::var_os("HOME")
            .map(|value| value.to_string_lossy().into_owned())
            .or_else(current_user_home)
    } else {
        passwd_home(user)
    };
    let Some(home) = home else {
        return token.to_owned();
    };
    let trimmed = home.trim_end_matches('/');
    if trimmed.is_empty() {
        return if suffix.is_empty() {
            "/".to_owned()
        } else {
            suffix.to_owned()
        };
    }
    format!("{trimmed}{suffix}")
}

fn current_user_home() -> Option<String> {
    crate::client::current_account_home()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

fn passwd_home(user: &str) -> Option<String> {
    const DEFAULT_BUFFER: usize = 16 * 1024;
    const MAX_BUFFER: usize = 16 * 1024 * 1024;

    // Besides being invalid input to `getpwnam_r`, an interior NUL could otherwise make the NSS
    // lookup apply to a different username than the policy code inspected.
    let name = CString::new(user).ok()?;
    // SAFETY: `sysconf` has no pointer arguments or caller-maintained invariants. A negative result
    // means that libc has no useful size recommendation, in which case the conservative default is
    // used.
    let suggested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut size = if suggested > 0 {
        usize::try_from(suggested)
            .ok()?
            .clamp(DEFAULT_BUFFER, MAX_BUFFER)
    } else {
        DEFAULT_BUFFER
    };

    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; size];
        // SAFETY: `name` is a live NUL-terminated C string. `record` points to writable storage,
        // and `buffer` is live and writable for exactly the length passed. `result` is an out
        // pointer. No field of `record` is read unless libc reports success and a non-null result.
        let code = unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE {
            if size >= MAX_BUFFER {
                return None;
            }
            size = size.saturating_mul(2).min(MAX_BUFFER);
            continue;
        }
        if code == libc::EINTR {
            continue;
        }
        if code != 0 || result.is_null() {
            return None;
        }
        // SAFETY: the successful non-null `getpwnam_r` result initializes `record`. `pw_dir`
        // points into either `record` or `buffer` and is NUL-terminated by the API contract. The
        // bytes are copied into an owned Vec/String before either backing allocation is dropped.
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() {
            return None;
        }
        let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes().to_vec();
        if bytes.is_empty() {
            return None;
        }
        return String::from_utf8(bytes).ok();
    }
}

/// Quote one argv as the exact POSIX shell word list admitted by policy.
///
/// Rendering is fallible because a tilde expansion can introduce terminal control characters
/// through ambient account data even when the original argv was clean.
pub fn render<S: AsRef<str>>(argv: &[S]) -> Result<String> {
    let mut quoted = Vec::with_capacity(argv.len());
    for token in argv {
        let expanded = expand_tilde(token.as_ref());
        if let Some(error) = control_refusal(&expanded, "command argument") {
            return Err(error);
        }
        quoted.push(quote(&expanded));
    }
    Ok(quoted.join(" "))
}

fn quote(token: &str) -> String {
    if token.is_empty() {
        return "''".to_owned();
    }
    if token.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    }) {
        return token.to_owned();
    }
    format!("'{}'", token.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_account() -> (String, String) {
        const MAX_BUFFER: usize = 16 * 1024 * 1024;
        // SAFETY: `getuid` has no preconditions.
        let uid = unsafe { libc::getuid() };
        let mut size = 16 * 1024;
        loop {
            let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
            let mut result = std::ptr::null_mut();
            let mut buffer = vec![0_u8; size];
            // SAFETY: every pointer names live writable storage of the advertised size. Fields are
            // read only after a successful non-null result and copied before the buffer is dropped.
            let code = unsafe {
                libc::getpwuid_r(
                    uid,
                    record.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if code == libc::ERANGE && size < MAX_BUFFER {
                size = size.saturating_mul(2).min(MAX_BUFFER);
                continue;
            }
            assert_eq!(code, 0, "getpwuid_r failed");
            assert!(!result.is_null(), "current account missing from NSS");
            // SAFETY: successful `getpwuid_r` initialized the record and both pointers remain live
            // until the strings below have been copied.
            let record = unsafe { record.assume_init() };
            assert!(!record.pw_name.is_null());
            assert!(!record.pw_dir.is_null());
            let user = unsafe { CStr::from_ptr(record.pw_name) }
                .to_str()
                .unwrap()
                .to_owned();
            let home = unsafe { CStr::from_ptr(record.pw_dir) }
                .to_str()
                .unwrap()
                .to_owned();
            return (user, home);
        }
    }

    #[test]
    fn intended_commands_are_admitted_with_program_prefix_and_subcommand() {
        let config = Config::default();
        let admission = admit("with-proxy git -C /tmp/repo log --oneline", &config).unwrap();
        assert_eq!(admission.prefix, vec!["with-proxy"]);
        assert_eq!(admission.program, "git");
        assert_eq!(admission.subcommand.as_deref(), Some("log"));
        assert!(admit("cargo fetch", &config).is_err());
    }

    #[test]
    fn tokenizer_matches_python_shlex_for_quotes_escapes_comments_and_empty_words() {
        let config = Config {
            allow: vec!["printf".into()],
            ..Config::default()
        };
        let admission = admit(
            "printf '' \"two words\" a\\ b '#literal' a#b \"x\\qx\"",
            &config,
        )
        .unwrap();
        assert_eq!(
            admission.argv,
            vec!["printf", "", "two words", "a b", "#literal", "a#b", "x\\qx"]
        );
    }

    #[test]
    fn metacharacters_are_literal_arguments_and_rendered_inert() {
        let admission = admit("git status; curl evil.example && id", &Config::default()).unwrap();
        assert_eq!(
            admission.argv,
            vec!["git", "status;", "curl", "evil.example", "&&", "id"]
        );
        assert_eq!(
            admission.rendered(),
            "git 'status;' curl evil.example '&&' id"
        );
    }

    #[test]
    fn render_is_byte_for_byte_python_shlex_quote_style() {
        assert_eq!(
            render(&["git", "commit", "-m", "two words", "it's", ""]).unwrap(),
            "git commit -m 'two words' 'it'\"'\"'s' ''"
        );
    }

    #[test]
    fn refuses_empty_unbalanced_paths_prefix_repetition_and_unknown_programs() {
        let config = Config::default();
        for (command, fragment) in [
            (" ", "empty command"),
            ("git 'no", "No closing quotation"),
            ("git x\\", "No escaped character"),
            ("./git status", "bare command name"),
            ("with-proxy with-proxy git status", "repeated"),
            ("curl x", "not allowlisted"),
        ] {
            let error = admit(command, &config).expect_err(command);
            assert_eq!(error.exit_code(), crate::error::EXIT_REFUSED);
            assert!(error.to_string().contains(fragment), "{error}");
        }
    }

    /// The wildcard turns off the PROGRAM-NAME check and nothing else.
    ///
    /// Naming each surviving rule matters more than checking that `curl` now runs: the value of an
    /// allow-everything mode a project can actually reach for is that it is still not a way to
    /// smuggle control characters into a shared pane or to execute a planted binary by path.
    #[test]
    fn the_allow_wildcard_admits_any_program_and_keeps_every_other_rule() {
        let config = Config {
            allow: vec!["*".into()],
            ..Config::default()
        };
        for (command, program) in [
            ("curl https://example.invalid", "curl"),
            ("bash -lc true", "bash"),
            ("rm -rf /x", "rm"),
        ] {
            assert_eq!(admit(command, &config).expect(command).program, program);
        }
        assert_eq!(
            admit("with-proxy curl https://example.invalid", &config)
                .unwrap()
                .prefix,
            vec!["with-proxy"]
        );
        for (command, fragment) in [
            ("/bin/curl x", "bare command name"),
            ("./curl x", "bare command name"),
            ("curl x\u{0007}", "terminal control U+0007"),
            ("with-proxy with-proxy curl x", "repeated"),
            ("git --exec-path=/tmp/evil status", "is denied for git"),
            ("git push --receive-pack=/tmp/evil origin", "is denied"),
            (
                "cargo build --release",
                "subcommand 'cargo build' is not allowlisted",
            ),
            ("with-proxy", "no program"),
        ] {
            let error = admit(command, &config).expect_err(command);
            assert_eq!(error.exit_code(), crate::error::EXIT_REFUSED, "{command}");
            assert!(error.to_string().contains(fragment), "{command}: {error}");
        }

        // The wildcard does not reach cargo's own guard: without a positive subcommand list,
        // cargo stays refused outright rather than inheriting "everything is allowed".
        let no_cargo_list = Config {
            allow: vec!["*".into()],
            allow_subcommand: std::collections::BTreeMap::new(),
            ..Config::default()
        };
        let error = admit("cargo fetch", &no_cargo_list).expect_err("cargo fetch");
        assert!(
            error
                .to_string()
                .contains("requires an explicit allow_subcommand entry"),
            "{error}"
        );
    }

    #[test]
    fn deny_rules_observe_global_subcommand_and_anywhere_scopes() {
        let config = Config::default();
        for command in [
            "git -c core.pager=sh log",
            "git --exec-path=/tmp/evil status",
            "gh extension exec evil",
            "git push --receive-pack=/tmp/evil origin main",
        ] {
            assert!(admit(command, &config).is_err(), "{command}");
        }
        assert_eq!(
            admit("git notes add -c deadbeef", &config)
                .unwrap()
                .subcommand
                .as_deref(),
            Some("notes")
        );
        assert_eq!(
            admit("git -C /tmp/repo log", &config)
                .unwrap()
                .subcommand
                .as_deref(),
            Some("log")
        );
    }

    #[test]
    fn distinct_prefixes_can_chain_but_each_can_appear_only_once() {
        let config = Config {
            prefixes: vec!["outer".into(), "inner".into()],
            ..Config::default()
        };
        assert_eq!(
            admit("outer inner git status", &config).unwrap().prefix,
            vec!["outer", "inner"]
        );
        assert!(admit("outer inner outer git status", &config).is_err());
    }

    #[test]
    fn leading_tilde_expands_and_remains_quoted() {
        let home = std::env::var("HOME").expect("test environment has HOME");
        let rendered = render(&["git", "-C", "~/dir; rm -rf /", "status"]).unwrap();
        assert!(rendered.contains(&home));
        assert!(rendered.contains('\''));
        assert!(!rendered.contains("~/"));
        assert_eq!(expand_tilde("a~b"), "a~b");
        assert_eq!(expand_tilde("--opt=~/x"), "--opt=~/x");
    }

    #[test]
    fn named_current_user_expands_through_nss() {
        let (user, home) = current_account();
        let expected_home = home.trim_end_matches('/');
        let expected = if expected_home.is_empty() {
            "/probe".to_owned()
        } else {
            format!("{expected_home}/probe")
        };
        assert_eq!(expand_tilde(&format!("~{user}/probe")), expected);
    }

    #[test]
    fn unknown_or_interior_nul_user_is_left_literal() {
        let unknown = format!("~__herdr_run_missing_user_{}/probe", std::process::id());
        assert_eq!(expand_tilde(&unknown), unknown);
        let nul = "~bad\0user/probe";
        assert_eq!(expand_tilde(nul), nul);
    }

    #[test]
    fn custom_policy_can_widen_narrow_and_replace_defaults() {
        let mut config = Config {
            allow: vec!["cargo".into()],
            ..Config::default()
        };
        config.prefixes.clear();
        assert_eq!(admit("cargo fetch", &config).unwrap().program, "cargo");
        assert!(admit("git status", &config).is_err());
        assert!(admit("with-proxy cargo fetch", &config).is_err());
    }

    #[test]
    fn cargo_is_disabled_by_default_and_explicit_opt_in_is_fail_closed() {
        assert!(admit("cargo fetch", &Config::default()).is_err());
        let mut config = Config::default();
        config.allow.push("cargo".to_owned());
        for command in [
            "cargo fetch",
            "with-proxy cargo fetch --manifest-path /w/Cargo.toml",
            "cargo update -p serde",
            "cargo generate-lockfile",
            "cargo vendor",
            "cargo metadata",
        ] {
            assert!(admit(command, &config).is_ok(), "{command}");
        }
        for command in [
            "cargo",
            "cargo build",
            "cargo test",
            "cargo run",
            "cargo bench",
            "cargo install ripgrep",
            "cargo rustc",
            "cargo clippy",
            "cargo doc",
            "cargo miri test",
            "cargo something-new",
            "with-proxy cargo build",
            "cargo --config build.rustc-wrapper=/tmp/evil fetch",
            "cargo --config=build.rustc-wrapper=/tmp/evil fetch",
            "cargo fetch --config build.rustc-wrapper=/tmp/evil",
            "cargo fetch --config=build.rustc-wrapper=/tmp/evil",
            "cargo -Z unstable-options fetch",
            "cargo -Zunstable-options fetch",
            "cargo fetch -Z unstable-options",
            "cargo fetch -Zunstable-options",
            "cargo fetch -Z=unstable-options",
            "cargo fetch -qZunstable-options",
        ] {
            assert!(admit(command, &config).is_err(), "{command}");
        }
    }

    #[test]
    fn cargo_opt_in_cannot_drop_its_positive_subcommand_policy() {
        let mut config = Config::default();
        config.allow.push("cargo".to_owned());
        config.allow_subcommand.clear();
        config
            .allow_subcommand
            .insert("custom-tool".to_owned(), vec!["inspect".to_owned()]);

        let error = admit("cargo build", &config).unwrap_err();
        assert!(error
            .message()
            .contains("requires an explicit allow_subcommand"));
    }

    #[test]
    fn cargo_minimum_injection_denies_cannot_be_removed() {
        let mut config = Config {
            allow: vec!["cargo".to_owned()],
            ..Config::default()
        };
        config.deny_global.clear();
        for command in ["cargo fetch --config=x", "cargo fetch -Zunstable-options"] {
            let error = admit(command, &config).unwrap_err();
            assert!(error.message().contains("denied"));
        }
    }

    #[test]
    fn project_can_explicitly_widen_the_cargo_subcommand_policy() {
        let mut config = Config::default();
        config.allow.push("cargo".to_owned());
        config.allow_subcommand.insert(
            "cargo".to_owned(),
            vec!["fetch".to_owned(), "build".to_owned()],
        );
        assert!(admit("cargo build", &config).is_ok());
    }

    #[test]
    fn every_terminal_control_range_is_refused_before_pty_injection() {
        for codepoint in (0x00..=0x1f).chain(0x7f..=0x9f) {
            let character = char::from_u32(codepoint).unwrap();
            let command = format!("git status{character}evil");
            let error = admit(&command, &Config::default()).expect_err("control must fail");
            assert_eq!(
                error.exit_code(),
                crate::error::EXIT_REFUSED,
                "U+{codepoint:04X}"
            );
            assert!(error.to_string().contains("terminal control"));

            let error = render(&["git", command.as_str()]).expect_err("render must fail");
            assert_eq!(error.exit_code(), crate::error::EXIT_REFUSED);
        }
    }
}
