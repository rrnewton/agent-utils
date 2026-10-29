//! Finding a registry's long-running agentctl services among the user's systemd units.
//!
//! A unit is one of them when its single `ExecStart=` command runs an agentctl binary (directly,
//! or as the command of a `sh -c` script, after an optional `exec`) whose `--registry` resolves to
//! this registry, with the subcommand `chat run`, `watch-agents`, or `run-headless NAME`.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use super::host::{Host, UnitState};
use super::unit::{shell_words, ExecCommand, UnitText};

/// Which kind of long-running service a unit is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// `agentctl chat run`: a chat bridge.
    Bridge,
    /// `agentctl watch-agents`: the agent watcher.
    Watcher,
    /// `agentctl run-headless NAME`: a headless agent's wrapper.
    Headless,
}

impl Kind {
    /// The subcommand, as a label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bridge => "chat run",
            Self::Watcher => "watch-agents",
            Self::Headless => "run-headless",
        }
    }
}

/// Where the agentctl binary sits in the unit's `ExecStart=` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BinarySite {
    /// The program itself: `argv[0]`.
    Program,
    /// The command of the `sh -c` script in `argv[script]`, at these byte offsets of the script.
    Script {
        /// Index of the script word in `argv`.
        script: usize,
        /// Byte offset of the binary word in the script.
        start: usize,
        /// Byte offset just past it.
        end: usize,
    },
}

/// The option tables the argument reader needs: for each subcommand path, the long options
/// that take a value (global options included).
pub trait OptionTables {
    /// Long option names (without `--`) that take a value under `path`, such as
    /// `["chat", "run"]`; the empty path is the top level.
    fn value_options(&self, path: &[&str]) -> HashSet<String>;
    /// The canonical long name of an option that may be spelled by an alias (`state` for
    /// `registry`); an unknown name is returned unchanged.
    fn canonical(&self, name: &str) -> String {
        name.to_owned()
    }
}

/// One discovered service.
#[derive(Clone, Debug)]
pub struct Service {
    /// The unit name.
    pub unit: String,
    /// What it runs.
    pub kind: Kind,
    /// What systemd says about it now.
    pub state: UnitState,
    /// `systemctl cat` output: the definition that is backed up and replayed.
    pub text: String,
    /// The parsed definition.
    pub parsed: UnitText,
    /// The one `ExecStart=` command.
    pub exec: ExecCommand,
    /// Where the agentctl binary is in it.
    pub site: BinarySite,
    /// The binary as written in the unit.
    pub binary_word: String,
    /// The binary as an absolute path (looked up on the unit's PATH when written bare).
    pub binary: PathBuf,
    /// The arguments after the binary, up to the first shell operator.
    pub args: Vec<String>,
    /// The unit's environment variables.
    pub environment: Vec<(String, String)>,
    /// The unit's working directory.
    pub working_directory: PathBuf,
    /// The registry, resolved.
    pub registry: PathBuf,
    /// `chat run --bridge-state`.
    pub bridge_state: Option<PathBuf>,
    /// The watcher's state directory: `--state-dir`, else its default.
    pub state_dir: Option<PathBuf>,
    /// The headless agent's name.
    pub agent: Option<String>,
    /// The redirection of the script, such as `2>>/state/logs/bridge.log`.
    pub log: Option<String>,
}

impl Service {
    /// The environment variable's value in the unit.
    #[must_use]
    pub fn env(&self, name: &str) -> Option<&str> {
        self.environment
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The bridge's plugin home: `AGENTCTL_HOME`, else `$HOME/.agentctl`.
    #[must_use]
    pub fn plugin_home(&self) -> Option<PathBuf> {
        if self.kind != Kind::Bridge {
            return None;
        }
        self.env(crate::plugins::AGENTCTL_HOME_ENV)
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.home().map(|home| home.join(".agentctl")))
    }

    fn home(&self) -> Option<PathBuf> {
        self.env("HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    }

    /// The directories the service keeps state in.
    #[must_use]
    pub fn state_dirs(&self) -> Vec<PathBuf> {
        match self.kind {
            Kind::Bridge => self.bridge_state.iter().cloned().collect(),
            Kind::Watcher => self.state_dir.iter().cloned().collect(),
            Kind::Headless => self.agent_directory().into_iter().collect(),
        }
    }

    /// The headless agent's directory in the registry, in whichever layout holds it.
    #[must_use]
    pub fn agent_directory(&self) -> Option<PathBuf> {
        let agent = self.agent.as_deref()?;
        let layout2 = self.registry.join("agents").join(agent);
        let layout1 = self.registry.join(agent);
        Some(if !layout2.exists() && layout1.exists() {
            layout1
        } else {
            layout2
        })
    }
}

/// Whether a program name is an agentctl binary: `agentctl`, or `agentctl-SUFFIX` for a pinned
/// copy, but not the Slack helpers that share the prefix.
#[must_use]
pub fn is_agentctl_name(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "agentctl"
        || (name.starts_with("agentctl-")
            && !name.starts_with("agentctl-slack-")
            && !name.starts_with("agentctl-test-"))
}

fn is_shell(path: &str) -> bool {
    matches!(
        path.rsplit('/').next().unwrap_or(path),
        "sh" | "bash" | "dash"
    )
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            && !name.as_bytes()[0].is_ascii_digit()
    })
}

/// Lexically normalized absolute path: `.` and `..` removed without touching the disk.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// The path a registry argument names, the same file whichever way it was spelled.
#[must_use]
pub fn registry_identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize(path))
}

/// Read one unit; `Ok(None)` when it is not an agentctl service of `registry`.
pub fn read_service(
    host: &dyn Host,
    tables: &dyn OptionTables,
    state: UnitState,
    registry: &Path,
) -> Result<Option<Service>, String> {
    if !state.exec_start.contains("agentctl") {
        return Ok(None);
    }
    let text = host.cat(&state.id)?;
    let parsed = UnitText::parse(&text);
    let commands = parsed.effective("Service", "ExecStart");
    if commands.len() != 1 {
        return Ok(None);
    }
    let mut exec = ExecCommand::parse(commands[0])?;
    if state.transient {
        // A transient file escapes `%` as `%%`; the words a manager runs have it once.
        for word in &mut exec.argv {
            *word = super::unit::unescape_transient(word)?;
        }
    }
    let Some((site, binary_word, args, log)) = locate(&exec)? else {
        return Ok(None);
    };
    let mut environment = parsed.environment()?;
    if state.transient {
        for (_, value) in &mut environment {
            *value = super::unit::unescape_transient(value)?;
        }
    }
    let home = environment
        .iter()
        .find(|(name, _)| name == "HOME")
        .map(|(_, value)| PathBuf::from(value))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let working_directory = match parsed.last("Service", "WorkingDirectory") {
        Some(directory) => {
            let directory = directory.trim_start_matches('-');
            if directory == "~" {
                home.clone()
            } else {
                PathBuf::from(directory)
            }
        }
        None => home.clone(),
    };
    let reading = read_arguments(&args, tables);
    let Some(kind) = reading.kind else {
        return Ok(None);
    };
    let base = working_directory.clone();
    let resolve = |value: &str| {
        let path = PathBuf::from(value);
        normalize(&if path.is_absolute() {
            path
        } else {
            base.join(path)
        })
    };
    let service_registry = resolve(reading.registry.as_deref().unwrap_or(".agentctl"));
    if registry_identity(&service_registry) != registry_identity(registry) {
        return Ok(None);
    }
    let binary = if binary_word.contains('/') {
        resolve(&binary_word)
    } else {
        let path = environment
            .iter()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".to_owned());
        std::env::split_paths(&path)
            .map(|directory| directory.join(&binary_word))
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| PathBuf::from(&binary_word))
    };
    let state_dir = match kind {
        Kind::Watcher => Some(match &reading.state_dir {
            Some(directory) => resolve(directory),
            None => environment
                .iter()
                .find(|(name, value)| name == "XDG_STATE_HOME" && value.starts_with('/'))
                .map(|(_, value)| PathBuf::from(value))
                .unwrap_or_else(|| home.join(".local/state"))
                .join("agentctl/agent-watch"),
        }),
        _ => None,
    };
    Ok(Some(Service {
        unit: state.id.clone(),
        kind,
        state,
        text,
        parsed,
        exec,
        site,
        binary_word,
        binary,
        args,
        environment,
        working_directory,
        registry: service_registry,
        bridge_state: reading.bridge_state.as_deref().map(resolve),
        state_dir,
        agent: reading.agent,
        log,
    }))
}

type Located = (BinarySite, String, Vec<String>, Option<String>);

/// Find the agentctl binary in a command, its arguments, and a script's redirection.
fn locate(exec: &ExecCommand) -> Result<Option<Located>, String> {
    let argv = &exec.argv;
    if is_agentctl_name(&argv[0]) {
        return Ok(Some((
            BinarySite::Program,
            argv[0].clone(),
            argv[1..].to_vec(),
            None,
        )));
    }
    if !is_shell(&argv[0]) || argv.len() < 3 {
        return Ok(None);
    }
    let flag = &argv[1];
    if !(flag.starts_with('-') && !flag.starts_with("--") && flag.ends_with('c')) {
        return Ok(None);
    }
    let script = &argv[2];
    let words = match shell_words(script) {
        Ok(words) => words,
        // A script that does not mention agentctl is not ours; one that does but cannot be read
        // safely is reported.
        Err(error) if script.contains("agentctl") => {
            return Err(format!("its sh -c script cannot be read safely: {error}"))
        }
        Err(_) => return Ok(None),
    };
    let mut index = 0;
    while index < words.len()
        && !words[index].operator
        && (words[index].text == "exec" || is_assignment(&words[index].text))
    {
        index += 1;
    }
    let Some(command) = words.get(index).filter(|word| !word.operator) else {
        return Ok(None);
    };
    if !is_agentctl_name(&command.text) {
        return Ok(None);
    }
    let mut args = Vec::new();
    let mut rest = words[index + 1..].iter();
    let mut log = None;
    for word in rest.by_ref() {
        if word.operator {
            if word.text.starts_with('>') || word.text.contains('>') {
                log = Some(word.text.clone());
            }
            break;
        }
        args.push(word.text.clone());
    }
    // A script that goes on to run something else after the service is not ours to rewrite.
    for word in rest {
        if !word.operator || !word.text.contains('>') {
            return Err(format!(
                "its sh -c script runs more than the agentctl command: {script:?}"
            ));
        }
        log.get_or_insert_with(|| word.text.clone());
    }
    Ok(Some((
        BinarySite::Script {
            script: 2,
            start: command.start,
            end: command.end,
        },
        command.text.clone(),
        args,
        log,
    )))
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Reading {
    kind: Option<Kind>,
    registry: Option<String>,
    bridge_state: Option<String>,
    state_dir: Option<String>,
    agent: Option<String>,
}

/// Read agentctl arguments with the real option tables, so the value of an option is never
/// taken for the subcommand.
fn read_arguments(args: &[String], tables: &dyn OptionTables) -> Reading {
    let mut reading = Reading::default();
    let mut path: Vec<String> = Vec::new();
    let mut positionals: Vec<String> = Vec::new();
    let mut values = tables.value_options(&[]);
    let mut index = 0;
    while index < args.len() {
        let word = &args[index];
        index += 1;
        let option = if word == "--" {
            positionals.extend(args[index..].iter().cloned());
            break;
        } else if let Some(long) = word.strip_prefix("--") {
            match long.split_once('=') {
                Some((name, value)) => Some((name.to_owned(), Some(value.to_owned()))),
                None if values.contains(long) => {
                    let value = args.get(index).cloned();
                    index += 1;
                    Some((long.to_owned(), value))
                }
                None => Some((long.to_owned(), None)),
            }
        } else if word.starts_with('-') && word.len() > 1 {
            None
        } else {
            // The subcommand path ends where its first positional begins.
            let mut candidate: Vec<String> = path.clone();
            candidate.push(word.clone());
            let candidate: Vec<&str> = candidate.iter().map(String::as_str).collect();
            let known = matches!(
                candidate.as_slice(),
                ["chat"] | ["chat", "run"] | ["watch-agents"] | ["run-headless"]
            );
            if known && positionals.is_empty() {
                values = tables.value_options(&candidate);
                path.push(word.clone());
            } else {
                positionals.push(word.clone());
            }
            None
        };
        if let Some((name, Some(value))) = option {
            match tables.canonical(&name).as_str() {
                "registry" => reading.registry = Some(value),
                "bridge-state" => reading.bridge_state = Some(value),
                "state-dir" => reading.state_dir = Some(value),
                _ => {}
            }
        }
    }
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    reading.kind = match path.as_slice() {
        ["chat", "run"] if reading.bridge_state.is_some() => Some(Kind::Bridge),
        ["watch-agents"] => Some(Kind::Watcher),
        ["run-headless"] if !positionals.is_empty() => {
            reading.agent = positionals.first().cloned();
            Some(Kind::Headless)
        }
        _ => None,
    };
    reading
}

/// Units that run agentctl but could not be read, with the reason.
pub type Unreadable = Vec<(String, String)>;

/// Every service of `registry`, in unit-name order, and the units that look like agentctl
/// services but could not be read.
pub fn discover(
    host: &dyn Host,
    tables: &dyn OptionTables,
    registry: &Path,
) -> Result<(Vec<Service>, Unreadable), String> {
    let units = host.service_units()?;
    let states = host.show(&units)?;
    let mut services = Vec::new();
    let mut unreadable = Vec::new();
    for state in states {
        let unit = state.id.clone();
        match read_service(host, tables, state, registry) {
            Ok(Some(service)) => services.push(service),
            Ok(None) => {}
            Err(error) => unreadable.push((unit, error)),
        }
    }
    services.sort_by(|left, right| left.unit.cmp(&right.unit));
    Ok((services, unreadable))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) struct Tables;

    impl OptionTables for Tables {
        fn value_options(&self, path: &[&str]) -> HashSet<String> {
            let mut names: HashSet<String> = ["registry", "state", "herdr-bin", "term-backend"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            let extra: &[&str] = match path {
                ["chat", "run"] | ["chat"] => &["bridge-state", "config"],
                ["watch-agents"] => &["state-dir", "interval", "stall-minutes", "coordinator"],
                ["run-headless"] => &["claude-bin", "approval-timeout"],
                _ => &[],
            };
            names.extend(extra.iter().map(|name| (*name).to_owned()));
            names
        }

        fn canonical(&self, name: &str) -> String {
            if name == "state" { "registry" } else { name }.to_owned()
        }
    }

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn reads_each_kind() {
        let bridge = read_arguments(
            &words("--registry /r chat run --bridge-state /s --offer-reply-command"),
            &Tables,
        );
        assert_eq!(bridge.kind, Some(Kind::Bridge));
        assert_eq!(bridge.registry.as_deref(), Some("/r"));
        assert_eq!(bridge.bridge_state.as_deref(), Some("/s"));
        let watcher = read_arguments(
            &words("--registry=/r watch-agents --state-dir /w --stall-minutes 60"),
            &Tables,
        );
        assert_eq!(watcher.kind, Some(Kind::Watcher));
        assert_eq!(watcher.state_dir.as_deref(), Some("/w"));
        let headless = read_arguments(
            &words("--registry /r run-headless --approval-timeout 600 coordinator"),
            &Tables,
        );
        assert_eq!(headless.kind, Some(Kind::Headless));
        assert_eq!(headless.agent.as_deref(), Some("coordinator"));
    }

    #[test]
    fn an_alias_of_registry_counts_as_the_registry() {
        let reading = read_arguments(&words("--state=/other watch-agents"), &Tables);
        assert_eq!(reading.registry.as_deref(), Some("/other"));
        let spaced = read_arguments(&words("--state /other watch-agents"), &Tables);
        assert_eq!(spaced.registry.as_deref(), Some("/other"));
        assert_eq!(spaced.kind, Some(Kind::Watcher));
    }

    #[test]
    fn an_option_value_is_never_the_subcommand() {
        let reading = read_arguments(&words("--herdr-bin watch-agents chat run"), &Tables);
        assert_eq!(reading.kind, None);
        let other = read_arguments(&words("--registry /r send coordinator hello"), &Tables);
        assert_eq!(other.kind, None);
    }

    #[test]
    fn locates_a_binary_inside_a_shell_script() {
        let exec = ExecCommand {
            prefix: String::new(),
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "exec /s/bin/agentctl-92f81d81 --registry /r watch-agents 2>>/s/logs/watch.log"
                    .into(),
            ],
        };
        let (site, word, args, log) = locate(&exec).unwrap().unwrap();
        assert_eq!(word, "/s/bin/agentctl-92f81d81");
        assert_eq!(args, words("--registry /r watch-agents"));
        assert_eq!(log.as_deref(), Some("2>>/s/logs/watch.log"));
        let BinarySite::Script { start, end, .. } = site else {
            panic!("expected a script site");
        };
        assert_eq!(&exec.argv[2][start..end], "/s/bin/agentctl-92f81d81");
    }

    #[test]
    fn refuses_a_script_that_runs_more_afterwards() {
        let exec = ExecCommand {
            prefix: String::new(),
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "/s/agentctl watch-agents; rm -rf /tmp/x".into(),
            ],
        };
        assert!(locate(&exec).is_err());
    }

    #[test]
    fn helper_names_are_not_agentctl() {
        assert!(is_agentctl_name("/x/agentctl"));
        assert!(is_agentctl_name("/x/agentctl-92f81d81"));
        assert!(!is_agentctl_name("/x/agentctl-slack-plugin"));
        assert!(!is_agentctl_name("/x/agentctl-slack-outbound"));
        assert!(!is_agentctl_name("/x/myagentctl"));
    }
}
