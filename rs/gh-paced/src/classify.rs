//! Classify one `gh` command line into a request class and a token cost.
//!
//! The classifier never touches the network. It is deliberately conservative: anything it does
//! not recognise as LOCAL, READ, SEARCH or GIT_CREDENTIAL is WRITE, so an alias, an extension, a
//! new `gh` subcommand, or an unparseable command line is paced as the most expensive class.

use crate::config::Config;
use serde::{Deserialize, Serialize};

/// Request class. Each non-LOCAL class has its own token bucket and hourly window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// No GitHub network traffic (help, version, completion, local config).
    Local,
    /// Reads from the core REST or GraphQL API.
    Read,
    /// Search API reads, which GitHub limits separately and more tightly.
    Search,
    /// Anything that may create or change content, and anything unrecognised.
    Write,
    /// `gh auth git-credential get`: one per git network operation.
    GitCredential,
}

impl Class {
    /// Lower-case name used in state, config and audit records.
    pub fn name(self) -> &'static str {
        match self {
            Class::Local => "local",
            Class::Read => "read",
            Class::Search => "search",
            Class::Write => "write",
            Class::GitCredential => "git_credential",
        }
    }

    /// Every class that has a budget.
    pub const PACED: [Class; 4] = [
        Class::Read,
        Class::Search,
        Class::Write,
        Class::GitCredential,
    ];
}

/// What one `gh api` invocation will do, as far as the command line says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiCall {
    /// HTTP method, upper case (explicit `-X`, or the implied GET/POST).
    pub method: String,
    /// Endpoint with any scheme, host and leading slash removed.
    pub endpoint: String,
    /// `--paginate` or `--slurp` was given.
    pub paginate: bool,
    /// `-i/--include` was given: gh prints the response status line and headers on stdout,
    /// which the wrapper then scans for pushback.
    pub include: bool,
}

/// The classifier's verdict for one command line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Classification {
    /// Request class.
    pub class: Class,
    /// Tokens charged against the class bucket and hourly window.
    pub cost: u32,
    /// Top-level command (`pr`, `api`, ...), or empty for a bare `gh`.
    pub family: String,
    /// Subcommand (`comment`, `view`, ...), when the family has subcommands.
    pub sub: Option<String>,
    /// Short command description for messages and audit (`pr comment`, `api GET repos/o/r`).
    pub command: String,
    /// Why this class was chosen.
    pub reason: String,
    /// Loud warnings to print before the call (expensive pagination, watch loops).
    pub warnings: Vec<String>,
    /// When set, the call must be refused before it reaches GitHub (exit 64).
    pub refusal: Option<String>,
    /// Details of a `gh api` call.
    pub api: Option<ApiCall>,
    /// Index in the argument vector where the arguments after the command words begin.
    pub rest_start: usize,
    /// Wall-clock bound for a polling command (watch loops). The wrapper kills the child when it
    /// runs this long, so a loop can never poll beyond the tokens it was charged.
    pub deadline_secs: Option<f64>,
}

const LOCAL_FAMILIES: &[&str] = &[
    "completion",
    "config",
    "alias",
    "help",
    "version",
    "accessibility",
    "a11y",
    "actions",
    "environment",
    "exit-codes",
    "formatting",
    "mintty",
    "reference",
];

/// Families whose second word is an argument, not a subcommand.
const NO_SUB_FAMILIES: &[&str] = &[
    "api",
    "browse",
    "status",
    "completion",
    "version",
    "credits",
    "help",
    "accessibility",
    "a11y",
    "actions",
    "environment",
    "exit-codes",
    "formatting",
    "mintty",
    "reference",
];

fn normalize_sub(sub: &str) -> &str {
    match sub {
        "ls" => "list",
        "co" => "checkout",
        "new" => "create",
        "rm" => "delete",
        other => other,
    }
}

/// READ subcommands per family (after alias normalisation).
fn read_subs(family: &str) -> &'static [&'static str] {
    match family {
        "pr" => &["view", "list", "status", "checks", "diff", "checkout"],
        "issue" => &["view", "list", "status"],
        "run" => &["view", "list", "download", "watch"],
        "workflow" => &["view", "list"],
        "repo" => &[
            "view",
            "list",
            "clone",
            "set-default",
            "gitignore",
            "license",
        ],
        "release" => &["view", "list", "download", "verify", "verify-asset"],
        "label" => &["list"],
        "gist" => &["view", "list", "clone"],
        "secret" => &["list"],
        "variable" => &["list", "get"],
        "cache" => &["list"],
        "ssh-key" | "gpg-key" => &["list"],
        "org" => &["list"],
        "project" => &["view", "list", "field-list", "item-list"],
        "ruleset" => &["view", "list", "check"],
        "attestation" => &["verify", "download", "trusted-root"],
        "codespace" => &["view", "list"],
        "extension" => &["list", "browse"],
        "agent-task" => &["view", "list"],
        _ => &[],
    }
}

/// Families that are gh's own commands (not an alias or an extension). The empty family is a
/// bare `gh --help`.
fn is_known_family(family: &str) -> bool {
    family.is_empty()
        || LOCAL_FAMILIES.contains(&family)
        || NO_SUB_FAMILIES.contains(&family)
        || !read_subs(family).is_empty()
        || matches!(family, "auth" | "search")
}

fn is_flag(token: &str) -> bool {
    token.len() > 1 && token.starts_with('-')
}

struct Lead {
    words: Vec<String>,
    rest: usize,
    /// Index just after the first command word (where an alias's or extension's own arguments
    /// begin).
    after_first: usize,
    unknown_flag: Option<String>,
}

fn wants_third_word(words: &[String]) -> bool {
    matches!(
        (words[0].as_str(), words[1].as_str()),
        ("auth", "git-credential") | ("repo", "autolink") | ("repo", "deploy-key")
    )
}

/// Collect the command words (`pr comment`, `auth git-credential get`) and where the rest starts.
fn lead(args: &[String]) -> Lead {
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    let mut unknown_flag = None;
    let mut after_first = None;
    while i < args.len() {
        let t = args[i].as_str();
        if is_flag(t) {
            if t == "-R" || t == "--repo" {
                i += 2;
                continue;
            }
            if t.starts_with("--repo=") || (t.starts_with("-R") && t.len() > 2) {
                i += 1;
                continue;
            }
            if matches!(t, "--help" | "-h" | "--version") {
                i += 1;
                continue;
            }
            unknown_flag = Some(t.to_string());
            break;
        }
        words.push(t.to_string());
        i += 1;
        if words.len() == 1 {
            after_first = Some(i);
        }
        if words.len() == 1 && NO_SUB_FAMILIES.contains(&words[0].as_str()) {
            break;
        }
        if words.len() == 2 && !wants_third_word(&words) {
            break;
        }
        if words.len() == 3 {
            break;
        }
    }
    let rest = i.min(args.len());
    Lead {
        words,
        rest,
        after_first: after_first.unwrap_or(rest),
        unknown_flag,
    }
}

/// `--help` as the first flag, or `-h` as the last token, after nothing but bare words.
fn is_help_request(args: &[String]) -> bool {
    match args.iter().position(|t| is_flag(t)) {
        Some(idx) => args[idx] == "--help" || (args[idx] == "-h" && idx + 1 == args.len()),
        None => false,
    }
}

/// Every value of a flag given as `-X v`, `-Xv`, `-X=v`, `--long v` or `--long=v`, in order.
///
/// gh (pflag) lets the last occurrence of a scalar flag win. Callers that turn a value into a
/// cost or a limit take the most conservative of all occurrences instead, so the answer does not
/// depend on which occurrence gh honours.
fn flag_values<'a>(rest: &'a [String], short: Option<char>, long: &str) -> Vec<&'a str> {
    let long_eq = format!("{long}=");
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let t = rest[i].as_str();
        if t == "--" {
            break;
        }
        if t == long {
            if let Some(v) = rest.get(i + 1) {
                out.push(v.as_str());
            }
            i += 2;
            continue;
        }
        if let Some(v) = t.strip_prefix(long_eq.as_str()) {
            out.push(v);
            i += 1;
            continue;
        }
        if let Some(c) = short {
            let short_flag = format!("-{c}");
            if t == short_flag {
                if let Some(v) = rest.get(i + 1) {
                    out.push(v.as_str());
                }
                i += 2;
                continue;
            }
            if let Some(v) = t.strip_prefix(short_flag.as_str()) {
                if !t.starts_with("--") {
                    out.push(v.strip_prefix('=').unwrap_or(v));
                }
            }
        }
        i += 1;
    }
    out
}

fn has_flag(rest: &[String], names: &[&str]) -> bool {
    for t in rest {
        if t == "--" {
            return false;
        }
        if names.contains(&t.as_str()) {
            return true;
        }
        if names
            .iter()
            .any(|n| n.starts_with("--") && t.starts_with(&format!("{n}=")))
        {
            return true;
        }
    }
    false
}

/// Parse a gh duration-ish interval value in seconds (`30`, `30s`, `2m`).
fn parse_seconds(value: &str) -> Option<f64> {
    let v = value.trim();
    let (num, mult) = if let Some(n) = v.strip_suffix('s') {
        (n, 1.0)
    } else if let Some(n) = v.strip_suffix('m') {
        (n, 60.0)
    } else {
        (v, 1.0)
    };
    num.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| n * mult)
}

fn local(family: &str, sub: Option<String>, command: String, reason: &str) -> Classification {
    Classification {
        class: Class::Local,
        cost: 0,
        family: family.to_string(),
        sub,
        command,
        reason: reason.to_string(),
        warnings: Vec::new(),
        refusal: None,
        api: None,
        rest_start: 0,
        deadline_secs: None,
    }
}

/// Classify a `gh` argument vector (without the `gh` program name).
pub fn classify(args: &[String], cfg: &Config) -> Classification {
    let mut c = classify_inner(args, cfg);
    c.rest_start = if c.class == Class::Local {
        args.len()
    } else if is_unknown_command(&c) {
        // An alias or extension: everything after its name is its own argument list, which may
        // carry body text, so none of it is treated as a command word.
        lead(args).after_first
    } else {
        lead(args).rest
    };
    c
}

fn classify_inner(args: &[String], cfg: &Config) -> Classification {
    if args.is_empty() {
        return local("", None, "gh".into(), "bare gh prints help");
    }
    if args.len() == 1 && (args[0] == "--version" || args[0] == "version") {
        return local(
            "version",
            None,
            "version".into(),
            "prints the local version",
        );
    }
    if is_help_request(args) {
        let words: Vec<&str> = args
            .iter()
            .take_while(|t| !is_flag(t))
            .map(String::as_str)
            .collect();
        let family = words.first().copied().unwrap_or("");
        // Only gh's own commands are known to print help for `--help`. An alias or an extension
        // receives the argument and may do anything with it (an alias ending in `-b` turns it
        // into a comment body), so those stay WRITE.
        if is_known_family(family) {
            return local(
                family,
                None,
                format!("{} --help", words.join(" ")).trim().to_string(),
                "help output is local",
            );
        }
    }
    let lead = lead(args);
    let rest = &args[lead.rest..];
    let words = &lead.words;
    let Some(family) = words.first().map(String::as_str) else {
        return fail_safe(
            "",
            None,
            "gh".into(),
            &format!(
                "unrecognised flag {} before the command",
                lead.unknown_flag.as_deref().unwrap_or("?")
            ),
        );
    };
    if LOCAL_FAMILIES.contains(&family) {
        return local(
            family,
            words.get(1).cloned(),
            words.join(" "),
            "no GitHub traffic",
        );
    }
    if family == "api" {
        return classify_api(rest, cfg);
    }
    if family == "browse" {
        return paced(
            Class::Read,
            1,
            family,
            None,
            "browse".into(),
            "opens a URL; may look up the repository",
        );
    }
    if family == "credits" {
        return paced(
            Class::Read,
            1,
            family,
            None,
            "credits".into(),
            "reads the contributor list",
        );
    }
    if family == "status" {
        return paced(
            Class::Search,
            3,
            family,
            None,
            "status".into(),
            "gh status runs several search and GraphQL queries (charged 3 search tokens)",
        );
    }
    let Some(raw_sub) = words.get(1).map(String::as_str) else {
        if let Some(flag) = &lead.unknown_flag {
            return fail_safe(
                family,
                None,
                family.to_string(),
                &format!("unrecognised flag {flag} before the subcommand"),
            );
        }
        return fail_safe(family, None, family.to_string(), "unknown command");
    };
    let sub = normalize_sub(raw_sub);
    let command = format!("{family} {sub}");
    let sub_owned = Some(sub.to_string());
    if family == "auth" {
        return match sub {
            "status" => paced(
                Class::Read,
                1,
                family,
                sub_owned,
                command,
                "validates the token against the API",
            ),
            "token" | "setup-git" | "switch" => {
                local(family, sub_owned, command, "local credential store only")
            }
            "git-credential" => match words.get(2).map(String::as_str) {
                Some("get") => paced(
                    Class::GitCredential,
                    1,
                    family,
                    sub_owned,
                    "auth git-credential get".into(),
                    "git asks for a credential before each network operation",
                ),
                Some(op @ ("store" | "erase")) => local(
                    family,
                    sub_owned,
                    format!("auth git-credential {op}"),
                    "gh ignores store and erase",
                ),
                _ => fail_safe(
                    family,
                    sub_owned,
                    command,
                    "unknown git-credential operation",
                ),
            },
            _ => fail_safe(
                family,
                sub_owned,
                command,
                "auth operation that may contact GitHub",
            ),
        };
    }
    if family == "search" {
        let mut c = paced(Class::Search, 1, family, sub_owned, command, "search API");
        apply_limit_cost(&mut c, rest);
        return c;
    }
    if family == "extension" && sub == "search" {
        let mut c = paced(
            Class::Search,
            1,
            family,
            sub_owned,
            command,
            "searches repositories",
        );
        apply_limit_cost(&mut c, rest);
        return c;
    }
    if family == "repo" && (sub == "autolink" || sub == "deploy-key") {
        let third = words.get(2).map(|w| normalize_sub(w)).unwrap_or("");
        let command = format!("{family} {sub} {third}").trim().to_string();
        if matches!(third, "list" | "view") {
            return paced(Class::Read, 1, family, sub_owned, command, "lists or views");
        }
        return fail_safe(family, sub_owned, command, "changes repository settings");
    }
    if read_subs(family).contains(&sub) {
        let mut c = paced(
            Class::Read,
            1,
            family,
            sub_owned,
            command,
            "read-only subcommand",
        );
        apply_limit_cost(&mut c, rest);
        apply_watch(&mut c, rest, cfg);
        return c;
    }
    let known_family = !read_subs(family).is_empty() || family == "auth";
    if known_family {
        return fail_safe(
            family,
            sub_owned,
            command,
            "subcommand that may create or change content",
        );
    }
    // The second word of an alias or extension is an argument of unknown meaning (it may be a
    // comment body or a token), so it is kept out of the command description and the audit.
    fail_safe(
        family,
        None,
        family.to_string(),
        "unknown command (alias, extension, or new gh command)",
    )
}

/// True when `c` is an alias, an extension, or a gh command this classifier does not know. The
/// audit then records flag names only and replaces every other argument with `<arg>`.
pub fn is_unknown_command(c: &Classification) -> bool {
    c.class != Class::Local
        && c.family != "api"
        && (c.family.is_empty() || !is_known_family(&c.family))
}

fn paced(
    class: Class,
    cost: u32,
    family: &str,
    sub: Option<String>,
    command: String,
    reason: &str,
) -> Classification {
    Classification {
        class,
        cost,
        family: family.to_string(),
        sub,
        command,
        reason: reason.to_string(),
        warnings: Vec::new(),
        refusal: None,
        api: None,
        rest_start: 0,
        deadline_secs: None,
    }
}

fn fail_safe(family: &str, sub: Option<String>, command: String, why: &str) -> Classification {
    paced(
        Class::Write,
        1,
        family,
        sub,
        command,
        &format!("{why}; classified WRITE (fail safe)"),
    )
}

/// `-L/--limit N` on a list or search: gh fetches up to 100 items per request.
fn apply_limit_cost(c: &mut Classification, rest: &[String]) {
    // The largest of all `--limit` values, whichever occurrence gh honours.
    let largest = flag_values(rest, Some('L'), "--limit")
        .into_iter()
        .filter_map(|v| v.trim().parse::<u64>().ok())
        .max();
    if let Some(n) = largest {
        let pages = n.div_ceil(100).max(1);
        let pages = u32::try_from(pages).unwrap_or(u32::MAX);
        if pages > c.cost {
            c.warnings.push(format!(
                "--limit {n} may fetch {pages} pages; charging {pages} {} tokens",
                c.class.name()
            ));
            c.cost = pages;
        }
    }
}

/// Watch loops poll GitHub for as long as they run.
///
/// The loop is charged `watch_cost` tokens up front and given a deadline that the purchased
/// tokens cover: `(cost / requests per poll) * interval`. `gh run watch` makes two requests per
/// poll (the run and its jobs); `gh pr checks --watch` makes one. The wrapper kills the child at
/// the deadline, so a watch can never poll beyond what it paid for.
fn apply_watch(c: &mut Classification, rest: &[String], cfg: &Config) {
    let (is_watch, default_interval, requests_per_poll) =
        match (c.family.as_str(), c.sub.as_deref()) {
            ("pr", Some("checks")) => (has_flag(rest, &["--watch"]), 10.0, 1.0),
            ("run", Some("watch")) => (true, 3.0, 2.0),
            _ => (false, 0.0, 1.0),
        };
    if !is_watch {
        return;
    }
    // The smallest of all `--interval` values, whichever occurrence gh honours.
    let interval = flag_values(rest, Some('i'), "--interval")
        .into_iter()
        .filter_map(parse_seconds)
        .reduce(f64::min)
        .unwrap_or(default_interval);
    c.cost = c.cost.max(cfg.watch_cost);
    let polls = (f64::from(c.cost) / requests_per_poll).floor().max(1.0);
    let deadline = polls * interval.max(1.0);
    c.deadline_secs = Some(deadline);
    c.warnings.push(format!(
        "{} polls GitHub every {interval} s until it finishes; charging {} read tokens up front, which cover {polls} polls, so it is stopped after {deadline} s",
        c.command, c.cost
    ));
    if interval < cfg.min_watch_interval_secs && !cfg.allow_fast_watch {
        c.refusal = Some(format!(
            "{} would poll every {interval} s, faster than the {} s minimum; pass `--interval {}` (or set GH_PACED_ALLOW_FAST_WATCH=1)",
            c.command, cfg.min_watch_interval_secs, cfg.min_watch_interval_secs
        ));
    }
}

/// Normalise an API endpoint: drop scheme and host, the leading slash, and the query string.
pub fn normalize_endpoint(endpoint: &str) -> String {
    let mut e = endpoint.trim();
    for scheme in ["https://", "http://"] {
        if let Some(after) = e.strip_prefix(scheme) {
            e = after.find('/').map(|i| &after[i..]).unwrap_or("");
        }
    }
    let e = e.trim_start_matches('/');
    let e = e.split(['?', '#']).next().unwrap_or("");
    e.to_string()
}

/// Parsed `gh api` arguments.
#[derive(Debug, Default, Clone)]
pub struct ApiArgs {
    /// Explicit `-X/--method` value.
    pub method: Option<String>,
    /// `-f/--raw-field key=value` pairs, verbatim.
    pub raw_fields: Vec<String>,
    /// `-F/--field key=value` pairs, verbatim (`@file` and `@-` read a file or stdin).
    pub typed_fields: Vec<String>,
    /// `--input FILE` (`-` = stdin).
    pub input: Option<String>,
    /// `--paginate`.
    pub paginate: bool,
    /// `--slurp`.
    pub slurp: bool,
    /// `-i/--include`.
    pub include: bool,
    /// Positional arguments; the first is the endpoint.
    pub positionals: Vec<String>,
    /// First flag the parser did not recognise.
    pub unknown: Option<String>,
}

const API_VALUE_SHORT: &[char] = &['X', 'H', 'f', 'F', 'q', 't', 'p'];
const API_BOOL_SHORT: &[char] = &['i', 'h'];
const API_VALUE_LONG: &[&str] = &[
    "method",
    "header",
    "raw-field",
    "field",
    "jq",
    "template",
    "preview",
    "input",
    "cache",
    "hostname",
];
const API_BOOL_LONG: &[&str] = &["include", "paginate", "silent", "slurp", "verbose", "help"];

fn api_store(a: &mut ApiArgs, name: &str, value: String) {
    match name {
        "X" | "method" => a.method = Some(value.to_ascii_uppercase()),
        "f" | "raw-field" => a.raw_fields.push(value),
        "F" | "field" => a.typed_fields.push(value),
        "input" => a.input = Some(value),
        _ => {}
    }
}

/// Parse the arguments that follow `gh api` with pflag semantics: `--flag value` consumes the
/// next argument even when it starts with `-`, short flags combine (`-iXPOST`), and `--` ends
/// flag parsing.
pub fn parse_api(rest: &[String]) -> ApiArgs {
    let mut a = ApiArgs::default();
    let mut i = 0;
    while i < rest.len() {
        let t = rest[i].as_str();
        i += 1;
        if t == "--" {
            a.positionals.extend(rest[i..].iter().cloned());
            break;
        }
        if let Some(long) = t.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            if API_VALUE_LONG.contains(&name) {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        let v = rest.get(i).cloned().unwrap_or_default();
                        i += 1;
                        v
                    }
                };
                api_store(&mut a, name, value);
            } else if API_BOOL_LONG.contains(&name) {
                match name {
                    "paginate" => a.paginate = true,
                    "slurp" => a.slurp = true,
                    "include" => a.include = true,
                    _ => {}
                }
            } else if a.unknown.is_none() {
                a.unknown = Some(t.to_string());
            }
            continue;
        }
        if is_flag(t) {
            let chars: Vec<char> = t[1..].chars().collect();
            let mut j = 0;
            while j < chars.len() {
                let c = chars[j];
                if API_VALUE_SHORT.contains(&c) {
                    let attached: String = chars[j + 1..].iter().collect();
                    let value = if attached.is_empty() {
                        let v = rest.get(i).cloned().unwrap_or_default();
                        i += 1;
                        v
                    } else {
                        attached.strip_prefix('=').unwrap_or(&attached).to_string()
                    };
                    api_store(&mut a, &c.to_string(), value);
                    break;
                } else if API_BOOL_SHORT.contains(&c) {
                    if c == 'i' {
                        a.include = true;
                    }
                    j += 1;
                } else {
                    if a.unknown.is_none() {
                        a.unknown = Some(t.to_string());
                    }
                    break;
                }
            }
            continue;
        }
        a.positionals.push(t.to_string());
    }
    a
}

/// True when `text` contains `word` delimited by non-identifier characters.
pub fn contains_word(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0;
    while let Some(pos) = text[start..].find(word) {
        let at = start + pos;
        let end = at + word.len();
        let before_ok = at == 0 || !is_ident(bytes[at - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        start = at + 1;
    }
    false
}

/// Largest file the classifier reads to inspect a GraphQL document.
pub const MAX_INSPECT_BYTES: u64 = 1 << 20;

/// Read a regular file of at most [`MAX_INSPECT_BYTES`] as UTF-8; anything else is `None`.
fn read_small(path: &str) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_INSPECT_BYTES {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut buf = String::new();
    file.take(MAX_INSPECT_BYTES + 1)
        .read_to_string(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_INSPECT_BYTES {
        return None;
    }
    Some(buf)
}

/// Every GraphQL document a `gh api graphql` call may send, or `None` when any part of the
/// request body cannot be inspected without consuming stdin.
///
/// With `--input`, gh sends that file as the request body and moves the fields into the URL; without
/// it, the fields form the body. Rather than model which one GitHub reads, every candidate is
/// collected and the call is READ only when none of them is a mutation:
///
/// - every `-f/-F query=...` value (a typed `@file` is read; `@-` is uninspectable);
/// - a key such as `query[...]`, which gh nests into an object, is uninspectable;
/// - `--input FILE` must be a JSON object with a string `query`; its raw text is also returned so
///   the word `mutation` anywhere in it (a second operation, an escaped name) counts;
/// - `--input -` is uninspectable.
fn graphql_documents(a: &ApiArgs) -> Option<Vec<String>> {
    let mut docs = Vec::new();
    for f in &a.raw_fields {
        let (key, value) = f.split_once('=').unwrap_or((f.as_str(), ""));
        if key == "query" {
            docs.push(value.to_string());
        } else if key.starts_with("query[") {
            return None;
        }
    }
    for f in &a.typed_fields {
        let (key, value) = f.split_once('=').unwrap_or((f.as_str(), ""));
        if key == "query" {
            match value.strip_prefix('@') {
                Some("-") => return None,
                Some(path) => docs.push(read_small(path)?),
                None => docs.push(value.to_string()),
            }
        } else if key.starts_with("query[") {
            return None;
        }
    }
    if let Some(input) = &a.input {
        if input == "-" {
            return None;
        }
        let text = read_small(input)?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        let query = value.as_object()?.get("query")?.as_str()?.to_string();
        docs.push(query);
        docs.push(text);
    }
    Some(docs)
}

/// True when `endpoint` (already normalised) is a GraphQL endpoint.
fn is_graphql(endpoint: &str) -> bool {
    endpoint == "graphql" || endpoint.ends_with("/graphql")
}

fn classify_api(rest: &[String], cfg: &Config) -> Classification {
    let a = parse_api(rest);
    let endpoint = normalize_endpoint(a.positionals.first().map(String::as_str).unwrap_or(""));
    let has_body = !a.raw_fields.is_empty() || !a.typed_fields.is_empty() || a.input.is_some();
    let method = a.method.clone().unwrap_or_else(|| {
        if has_body {
            "POST".into()
        } else {
            "GET".into()
        }
    });
    let paginate = a.paginate || a.slurp;
    let api = ApiCall {
        method: method.clone(),
        endpoint: endpoint.clone(),
        paginate,
        include: a.include,
    };
    let command = format!("api {method} {endpoint}").trim().to_string();
    let mut c = paced(Class::Write, 1, "api", None, command, "");
    c.api = Some(api);
    if let Some(flag) = &a.unknown {
        c.reason = format!("unrecognised gh api flag {flag}; classified WRITE (fail safe)");
        return c;
    }
    let read_method = matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS");
    if is_graphql(&endpoint) {
        // Inspected for every method: `-X GET graphql -f query=mutation{...}` is still a
        // mutation as far as this wrapper is concerned.
        match graphql_documents(&a) {
            Some(docs) if docs.is_empty() => {
                c.reason =
                    "GraphQL request with no query to inspect; classified WRITE (fail safe)".into()
            }
            Some(docs) if docs.iter().all(|d| !contains_word(d, "mutation")) => {
                c.class = Class::Read;
                c.reason = "GraphQL query without a mutation".into();
            }
            Some(_) => c.reason = "GraphQL mutation".into(),
            None => {
                c.reason =
                    "GraphQL request whose body cannot be inspected; classified WRITE (fail safe)"
                        .into()
            }
        }
    } else if read_method {
        if endpoint.starts_with("search/") {
            c.class = Class::Search;
            c.reason = format!("{method} on the search API");
        } else if endpoint == "rate_limit" {
            c.class = Class::Read;
            c.reason =
                "GET /rate_limit (free on the primary limit, counted by secondary limits)".into();
        } else {
            c.class = Class::Read;
            c.reason = format!("{method} request");
        }
    } else if a.method.is_some() {
        c.reason = format!("explicit {method} request");
    } else {
        c.reason = "fields or --input without -X GET make gh send a POST".into();
    }
    if paginate {
        // A paginated write (a GraphQL mutation with a cursor, a POST that follows Link
        // headers) can repeat the write once per page, so it pays the same N tokens as a read.
        c.cost = cfg.paginate_cost.max(1);
        c.warnings.push(format!(
            "--paginate fetches pages back to back; charging {} {} tokens for this one invocation",
            c.cost,
            c.class.name()
        ));
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cls(line: &str) -> Classification {
        let args: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        classify(&args, &Config::default())
    }

    fn class_of(line: &str) -> Class {
        cls(line).class
    }

    #[test]
    fn local_commands() {
        for line in [
            "",
            "--version",
            "version",
            "help",
            "help pr create",
            "help environment",
            "pr --help",
            "pr create --help",
            "api --help",
            "pr comment 12 -h",
            "completion -s bash",
            "config get git_protocol",
            "config set editor vim",
            "alias list",
            "alias set co 'pr checkout'",
            "auth token",
            "auth setup-git",
            "auth switch --user someone",
            "auth git-credential store",
            "auth git-credential erase",
            "environment",
            "exit-codes",
            "formatting",
            "reference",
            "actions",
            "mintty",
            "accessibility",
        ] {
            assert_eq!(class_of(line), Class::Local, "{line:?}");
        }
    }

    #[test]
    fn help_detection_is_not_fooled_by_a_flag_value() {
        // `-b --help` makes `--help` the body, so gh would post a comment.
        assert_eq!(class_of("pr comment 12 -b --help"), Class::Write);
        // `-h` that is not the last token is not treated as help.
        assert_eq!(class_of("pr comment -h 12 -b x"), Class::Write);
        // An alias or extension receives `--help` as an argument and may still write.
        assert_eq!(class_of("my-alias --help"), Class::Write);
        assert_eq!(class_of("my-alias 12 --help"), Class::Write);
        assert_eq!(class_of("some-extension -h"), Class::Write);
        assert_eq!(cls("my-alias --help").command, "my-alias");
    }

    #[test]
    fn read_commands() {
        for line in [
            "pr view 12",
            "pr list --state open",
            "pr ls",
            "pr status",
            "pr checks 12",
            "pr diff 12",
            "pr checkout 12",
            "pr co 12",
            "-R owner/repo pr view 12",
            "pr -R owner/repo list",
            "issue view 3",
            "issue list",
            "issue status",
            "run view 99",
            "run list",
            "run download 99",
            "workflow list",
            "workflow view ci.yml",
            "repo view",
            "repo list",
            "repo clone owner/repo",
            "repo gitignore list",
            "repo license view mit",
            "repo autolink list",
            "repo deploy-key list",
            "release list",
            "release view v1",
            "release download v1",
            "label list",
            "gist list",
            "gist view abc",
            "secret list",
            "variable list",
            "variable get NAME",
            "cache list",
            "ssh-key list",
            "gpg-key list",
            "org list",
            "project list",
            "project view 1",
            "ruleset list",
            "ruleset check",
            "attestation verify file",
            "auth status",
            "browse 12",
            "extension list",
            "api repos/owner/repo",
            "api /repos/owner/repo/pulls?state=open",
            "api -X GET repos/owner/repo/issues -f state=open",
            "api --method=get repos/owner/repo",
            "api -XGET repos/o/r/issues -F per_page=100",
            "api -X HEAD repos/o/r",
            "api https://api.github.com/repos/o/r",
            "api rate_limit",
            "api graphql -f query={viewer{login}}",
        ] {
            assert_eq!(class_of(line), Class::Read, "{line:?}");
        }
    }

    #[test]
    fn search_commands() {
        for line in [
            "search issues foo",
            "search prs --repo o/r bar",
            "search code baz",
            "search repos x",
            "search commits y",
            "api search/issues?q=foo",
            "api -X GET search/code -f q=bar",
            "extension search thing",
            "status",
        ] {
            assert_eq!(class_of(line), Class::Search, "{line:?}");
        }
        assert_eq!(cls("status").cost, 3);
    }

    #[test]
    fn write_commands() {
        for line in [
            "api -X POST repos/o/r/issues/1/comments -f body=hi",
            "api --method PATCH repos/o/r/issues/1",
            "api --method=put repos/o/r/contents/x",
            "api -XDELETE repos/o/r/labels/x",
            "api repos/o/r/issues/1/comments -f body=hi",
            "api repos/o/r/issues/1/comments -F body=@file.md",
            "api repos/o/r/issues/1/comments --raw-field body=hi",
            "api repos/o/r/issues/1/comments --field body=hi",
            "api --method POST repos/o/r/issues/1/comments --input request.json",
            "api repos/o/r/issues/1/comments --input -",
            "api graphql -f query=mutation{addComment}",
            "api graphql -F query=@-",
            "api graphql --input -",
            "api --unknown-flag repos/o/r",
            "pr create --title t --body b",
            "pr new",
            "pr comment 1 -b hi",
            "pr edit 1 --title t",
            "pr merge 1 --squash",
            "pr close 1",
            "pr reopen 1",
            "pr review 1 --approve",
            "pr ready 1",
            "pr lock 1",
            "pr update-branch 1",
            "issue create -t t -b b",
            "issue comment 1 -b hi",
            "issue edit 1 --add-label x",
            "issue close 1",
            "issue reopen 1",
            "issue delete 1",
            "issue transfer 1 o/r",
            "issue lock 1",
            "issue pin 1",
            "label create bug",
            "label edit bug",
            "label delete bug",
            "label clone o/r",
            "release create v1",
            "release edit v1",
            "release delete v1",
            "release upload v1 file",
            "workflow run ci.yml",
            "workflow enable ci.yml",
            "workflow disable ci.yml",
            "run rerun 1",
            "run cancel 1",
            "run delete 1",
            "repo create x",
            "repo edit --description d",
            "repo delete o/r",
            "repo fork o/r",
            "repo sync",
            "repo rename x",
            "repo archive o/r",
            "repo autolink create",
            "repo deploy-key add key.pub",
            "gist create file.txt",
            "gist edit abc",
            "gist delete abc",
            "secret set NAME",
            "secret delete NAME",
            "variable set NAME",
            "variable delete NAME",
            "cache delete key",
            "ssh-key add k.pub",
            "gpg-key delete 1",
            "project create",
            "project item-add 1",
            "auth login",
            "auth logout",
            "auth refresh",
            "auth git-credential",
            "extension install o/gh-x",
            "codespace create",
            "my-alias",
            "some-extension arg",
            "--unknown pr view",
            "pr --state open list",
            "copilot suggest x",
        ] {
            assert_eq!(class_of(line), Class::Write, "{line:?}");
        }
    }

    #[test]
    fn git_credential_get() {
        assert_eq!(class_of("auth git-credential get"), Class::GitCredential);
        assert_eq!(
            cls("auth git-credential get").command,
            "auth git-credential get"
        );
    }

    #[test]
    fn paginate_and_limit_costs() {
        let c = cls("api --paginate repos/o/r/issues");
        assert_eq!((c.class, c.cost), (Class::Read, 10));
        assert!(!c.warnings.is_empty());
        let c = cls("api --slurp --paginate repos/o/r/issues");
        assert_eq!(c.cost, 10);
        let c = cls("api -X GET --paginate search/issues -f q=x");
        assert_eq!((c.class, c.cost), (Class::Search, 10));
        assert_eq!(cls("pr list -L 1000").cost, 10);
        assert_eq!(cls("pr list --limit=250").cost, 3);
        assert_eq!(cls("pr list -L50").cost, 1);
        assert_eq!(cls("search issues x --limit 300").cost, 3);
        // A paginated write can repeat once per page, so it pays the same N tokens as a read.
        let c = cls("api --paginate -X POST repos/o/r/x");
        assert_eq!((c.class, c.cost), (Class::Write, 10));
        assert!(!c.warnings.is_empty());
        let c = cls("api graphql --paginate -f query=mutation{x}");
        assert_eq!((c.class, c.cost), (Class::Write, 10));
    }

    /// gh honours the last occurrence of a scalar flag; gh-paced takes the most conservative of
    /// all occurrences, so neither order escapes the charge or the refusal.
    #[test]
    fn repeated_flags_use_the_most_conservative_value() {
        assert_eq!(cls("pr list --limit 1 --limit 1000").cost, 10);
        assert_eq!(cls("pr list --limit 1000 --limit 1").cost, 10);
        assert_eq!(cls("pr list -L 1 --limit=1000").cost, 10);
        assert!(cls("run watch 1 --interval 30 --interval 1")
            .refusal
            .is_some());
        assert!(cls("run watch 1 -i 1 --interval 30").refusal.is_some());
        assert!(cls("pr checks 1 --watch --interval 60 -i 2")
            .refusal
            .is_some());
        assert!(cls("run watch 1 -i 30 --interval 45").refusal.is_none());
    }

    #[test]
    fn watch_loops_are_charged_and_fast_ones_refused() {
        let c = cls("pr checks 12 --watch");
        assert_eq!(c.cost, 20);
        assert!(c.refusal.is_some(), "default 10 s interval is too fast");
        let c = cls("pr checks 12 --watch --interval 60");
        assert!(c.refusal.is_none());
        assert_eq!(c.cost, 20);
        let c = cls("run watch 99");
        assert!(c.refusal.is_some(), "default 3 s interval is too fast");
        let c = cls("run watch 99 -i 30");
        assert!(c.refusal.is_none());
        // 20 tokens buy 10 polls of 2 requests each, 30 s apart: stopped after 300 s.
        assert_eq!(c.deadline_secs, Some(300.0));
        // 20 tokens buy 20 polls of 1 request each, 30 s apart: stopped after 600 s.
        assert_eq!(
            cls("pr checks 12 --watch --interval 30").deadline_secs,
            Some(600.0)
        );
        let c = cls("pr checks 12");
        assert_eq!(c.cost, 1);
        assert!(c.refusal.is_none());
        assert_eq!(c.deadline_secs, None);
        assert_eq!(cls("pr view 12").deadline_secs, None);
        let cfg = Config {
            allow_fast_watch: true,
            ..Config::default()
        };
        let args: Vec<String> = ["run", "watch", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(classify(&args, &cfg).refusal.is_none());
    }

    #[test]
    fn api_parser_follows_pflag_rules() {
        let args: Vec<String> = ["-iXPOST", "repos/o/r", "-f", "-body=x", "--", "--input"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = parse_api(&args);
        assert_eq!(a.method.as_deref(), Some("POST"));
        assert!(a.include, "-i in a combined short group");
        assert_eq!(a.raw_fields, vec!["-body=x".to_string()]);
        assert_eq!(
            a.positionals,
            vec!["repos/o/r".to_string(), "--input".to_string()]
        );
        assert!(a.input.is_none());
        let args: Vec<String> = ["-H", "-X", "repos/o/r"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = parse_api(&args);
        assert!(a.method.is_none(), "-X here is the value of -H");
        assert!(!a.include);
        assert!(cls("api --include repos/o/r").api.expect("api").include);
        assert!(!cls("api repos/o/r").api.expect("api").include);
        assert!(
            !cls("api -H -i repos/o/r").api.expect("api").include,
            "-i here is the value of -H"
        );
        assert_eq!(normalize_endpoint("/repos/o/r?x=1"), "repos/o/r");
        assert_eq!(
            normalize_endpoint("https://api.github.com/graphql"),
            "graphql"
        );
    }

    #[test]
    fn graphql_query_files_are_inspected() {
        let dir = std::env::temp_dir().join(format!("gh-paced-gql-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let q = dir.join("q.graphql");
        std::fs::write(&q, "query { viewer { login } }").unwrap();
        let m = dir.join("m.graphql");
        std::fs::write(
            &m,
            "mutation { addComment(input: {}) { clientMutationId } }",
        )
        .unwrap();
        let inp = dir.join("in.json");
        std::fs::write(&inp, r#"{"query": "mutation X { y }"}"#).unwrap();
        let line = |extra: &str| -> Class {
            let mut args = vec!["api".to_string(), "graphql".to_string()];
            args.extend(extra.split_whitespace().map(str::to_string));
            classify(&args, &Config::default()).class
        };
        assert_eq!(line(&format!("-F query=@{}", q.display())), Class::Read);
        assert_eq!(line(&format!("-F query=@{}", m.display())), Class::Write);
        assert_eq!(line(&format!("--input {}", inp.display())), Class::Write);
        assert_eq!(line("-F query=@/nonexistent/file"), Class::Write);
        // gh sends the --input file as the request body (fields move to the URL), so a query
        // field cannot vouch for it: stdin cannot be inspected, and a mutation file is a write.
        assert_eq!(
            line("-f query=query{viewer{login}} --input -"),
            Class::Write
        );
        assert_eq!(
            line(&format!("-f query=query{{x}} --input {}", inp.display())),
            Class::Write
        );
        let qin = dir.join("qin.json");
        std::fs::write(&qin, r#"{"query": "query { viewer { login } }"}"#).unwrap();
        assert_eq!(line(&format!("--input {}", qin.display())), Class::Read);
        // Every query value counts, whichever gh keeps.
        assert_eq!(line("-f query=query{x} -f query=mutation{y}"), Class::Write);
        assert_eq!(line("-f query=mutation{y} -f query=query{x}"), Class::Write);
        // GraphQL is inspected whatever the method.
        assert_eq!(line("-X GET -f query=mutation{y}"), Class::Write);
        assert_eq!(line("-X GET -f query=query{y}"), Class::Read);
        // No query at all: nothing to vouch for a read.
        assert_eq!(line(""), Class::Write);
        assert!(contains_word("mutation{x}", "mutation"));
        assert!(!contains_word("query { mutations }", "mutation"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
