//! Escalation delivery for the team-restore driver (#1417): hand an
//! incident to a human through ordered, operator-configured commands.
//!
//! When the driver cannot bring a team back, it escalates: it tries each
//! configured route in order until one exits 0. A route is a plain argv,
//! never a shell string. The incident text is fed to the command on stdin
//! (see [`crate::restore::Host::run_stdin`]), and `{host}`, `{team}`, and
//! `{incident}` in every argv element are replaced with the incident's
//! fields, so one shared route file can name the team it pages about
//! without this module knowing any team, host, or person.
//!
//! Routes live in an operator-owned file in the synced configuration
//! directory, by default its `escalation.toml` (see
//! [`default_escalation_config_path`]). There is no TOML parser in the
//! workspace, so the file is a deliberately small frozen subset, refused
//! rather than misread when it steps outside it:
//!
//! ```toml
//! # A comment, whole-line or trailing.
//! [[route]]
//! argv = ["/path/to/notify", "--team", "{team}"]
//! timeout_seconds = 30
//!
//! [[route]]
//! argv = ["/path/to/fallback", "{host}", "{incident}"]
//! ```
//!
//! Only blank lines, comments, `[[route]]` headers, `argv` (a one-line
//! JSON array of strings, with JSON string escaping), and the optional
//! `timeout_seconds` (a plain positive decimal integer, default 30)
//! are supported. An absent file is an empty configuration, which
//! [`deliver`] reports as [`Delivery::Unconfigured`], distinctly from
//! every route failing. An unreadable or malformed file is an error
//! naming the file and the line, so a typo never silently disarms the
//! escalation.
//!
//! When no route delivers, the driver writes an alarm file ([`write_alarm`])
//! so the failure is visible on disk instead of only in a log nobody reads,
//! and clears it ([`clear_alarm`]) once an escalation gets through. Retries
//! wait per [`backoff_seconds`].

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::agent::{AgentError, AgentResult};
use crate::restore::{CommandOutcome, Host};

/// Schema of the alarm file [`write_alarm`] writes.
pub const ALARM_SCHEMA: &str = "agentctl-team-alarm/v1";
/// File name of the alarm inside its directory.
pub const ALARM_FILE: &str = "alarm.json";
/// A route's timeout when its `timeout_seconds` is absent.
pub const DEFAULT_ROUTE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest snippet of one failed route's output kept in a failure detail.
const DETAIL_CHARS: usize = 300;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

fn fail_at(path: &Path, line: usize, message: impl Into<String>) -> AgentError {
    fail(format!("{}:{line}: {}", path.display(), message.into()))
}

/// One escalation route: a plain command, tried in file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// The command and its arguments, first element the program. Never run
    /// through a shell. `{host}`, `{team}`, and `{incident}` are replaced
    /// in every element at delivery time.
    pub argv: Vec<String>,
    /// How long the command may run before the route counts as failed.
    pub timeout: Duration,
}

impl Route {
    /// A route running `argv` with the default timeout.
    #[must_use]
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            timeout: DEFAULT_ROUTE_TIMEOUT,
        }
    }
}

/// The operator's escalation routes, in the order they are tried. An empty
/// configuration is unconfigured, not failing: see [`Delivery`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EscalationConfig {
    /// Routes in try order; the first to exit 0 delivers.
    pub routes: Vec<Route>,
}

/// One incident an escalation is about. Every value comes from the caller;
/// this module hardcodes no team, host, or person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incident {
    /// The host the incident is on; replaces `{host}` in route argv.
    pub host: String,
    /// The team the incident concerns; replaces `{team}`.
    pub team: String,
    /// The incident's identity within the team; replaces `{incident}`.
    pub incident: String,
    /// The incident text, fed to each route command on stdin.
    pub text: String,
}

/// How one [`deliver`] attempt ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// A route exited 0; the incident reached a human.
    Delivered {
        /// Index of the delivering route in [`EscalationConfig::routes`].
        route_index: usize,
    },
    /// Every configured route was tried and none exited 0.
    Failed {
        /// One entry per route saying how it ended, joined with `; `.
        detail: String,
    },
    /// No routes are configured, so nothing was tried. Distinct from
    /// [`Delivery::Failed`]: there is no route to fix, only one to write.
    Unconfigured,
}

/// The default escalation route file: `escalation.toml` in the synced
/// configuration directory under `$XDG_CONFIG_HOME` when that is set to a
/// non-empty absolute path, else under `$HOME/.config`. `None` when
/// neither is set.
#[must_use]
pub fn default_escalation_config_path() -> Option<PathBuf> {
    escalation_config_path_from(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn escalation_config_path_from(
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(xdg) =
        xdg_config_home.filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
    {
        return Some(xdg.join("agent-utils").join("escalation.toml"));
    }
    home.filter(|path| !path.as_os_str().is_empty())
        .map(|home| {
            home.join(".config")
                .join("agent-utils")
                .join("escalation.toml")
        })
}

/// Read the escalation routes at `path`. An absent file is an empty
/// configuration (the [`Delivery::Unconfigured`] path); an unreadable or
/// malformed file is an error, with the file and line named for malformed
/// content, so a broken route file is loud instead of silently empty.
pub fn load_escalation_config(path: &Path) -> AgentResult<EscalationConfig> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(EscalationConfig::default());
        }
        Err(error) => {
            return Err(fail(format!("cannot read {}: {error}", path.display())));
        }
    };
    parse_escalation_config(&text, path)
}

/// One `[[route]]` section being read.
struct RouteBuilder {
    header_line: usize,
    argv: Option<Vec<String>>,
    timeout: Option<Duration>,
}

impl RouteBuilder {
    fn finish(self, path: &Path) -> AgentResult<Route> {
        let Some(argv) = self.argv else {
            return Err(fail_at(path, self.header_line, "[[route]] has no argv"));
        };
        Ok(Route {
            argv,
            timeout: self.timeout.unwrap_or(DEFAULT_ROUTE_TIMEOUT),
        })
    }
}

/// The frozen-subset parser behind [`load_escalation_config`]; see the
/// module docs for the subset. Anything outside it is refused here.
fn parse_escalation_config(text: &str, path: &Path) -> AgentResult<EscalationConfig> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut routes: Vec<Route> = Vec::new();
    let mut current: Option<RouteBuilder> = None;
    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[route]]" {
            if let Some(builder) = current.take() {
                routes.push(builder.finish(path)?);
            }
            current = Some(RouteBuilder {
                header_line: line_number,
                argv: None,
                timeout: None,
            });
            continue;
        }
        if line.starts_with('[') {
            return Err(fail_at(
                path,
                line_number,
                format!("unknown section {line:?}; only [[route]] is supported"),
            ));
        }
        let Some(builder) = current.as_mut() else {
            return Err(fail_at(
                path,
                line_number,
                "entry outside a [[route]] section",
            ));
        };
        let Some((key, value)) = line.split_once('=') else {
            return Err(fail_at(path, line_number, "expected KEY = VALUE"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "argv" => {
                if builder.argv.is_some() {
                    return Err(fail_at(
                        path,
                        line_number,
                        "duplicate argv in one [[route]]",
                    ));
                }
                let argv: Vec<String> = serde_json::from_str(value).map_err(|error| {
                    fail_at(
                        path,
                        line_number,
                        format!("argv must be a one-line JSON array of strings: {error}"),
                    )
                })?;
                if argv.first().is_none_or(|program| program.is_empty()) {
                    return Err(fail_at(
                        path,
                        line_number,
                        "argv must name a program as its first element",
                    ));
                }
                builder.argv = Some(argv);
            }
            "timeout_seconds" => {
                if builder.timeout.is_some() {
                    return Err(fail_at(
                        path,
                        line_number,
                        "duplicate timeout_seconds in one [[route]]",
                    ));
                }
                let seconds: u64 = value.parse().map_err(|_| {
                    fail_at(
                        path,
                        line_number,
                        format!("timeout_seconds must be a positive integer, not {value:?}"),
                    )
                })?;
                if seconds == 0 {
                    return Err(fail_at(
                        path,
                        line_number,
                        "timeout_seconds must be positive",
                    ));
                }
                builder.timeout = Some(Duration::from_secs(seconds));
            }
            "" => return Err(fail_at(path, line_number, "empty key")),
            other => {
                return Err(fail_at(
                    path,
                    line_number,
                    format!("unknown key {other:?} in [[route]]; supported keys are argv and timeout_seconds"),
                ));
            }
        }
    }
    if let Some(builder) = current.take() {
        routes.push(builder.finish(path)?);
    }
    Ok(EscalationConfig { routes })
}

/// `line` without its comment: the first `#` outside a double-quoted string
/// starts one, so a `#` inside an `argv` element stays.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in line.bytes().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b'#' {
            return &line[..index];
        }
    }
    line
}

/// Try each configured route in order until one exits 0. Every route runs
/// through [`Host::run_stdin`] with no working directory, the route's own
/// timeout, and `incident.text` on stdin; `{host}`, `{team}`, and
/// `{incident}` are substituted in every argv element first. An empty
/// configuration is [`Delivery::Unconfigured`] and runs nothing.
#[must_use]
pub fn deliver(host: &dyn Host, config: &EscalationConfig, incident: &Incident) -> Delivery {
    if config.routes.is_empty() {
        return Delivery::Unconfigured;
    }
    let mut failures: Vec<String> = Vec::new();
    for (route_index, route) in config.routes.iter().enumerate() {
        if route.argv.is_empty() {
            failures.push(format!("route {route_index}: empty argv"));
            continue;
        }
        let argv: Vec<String> = route
            .argv
            .iter()
            .map(|element| substitute(element, incident))
            .collect();
        let outcome = host.run_stdin(&argv, None, route.timeout, &incident.text);
        if outcome.status == Some(0) {
            return Delivery::Delivered { route_index };
        }
        failures.push(failure_detail(route_index, &outcome));
    }
    Delivery::Failed {
        detail: failures.join("; "),
    }
}

/// `template` with `{host}`, `{team}`, and `{incident}` replaced, in one
/// pass: a substituted value is never rescanned, and any other brace text
/// is left as written.
fn substitute(template: &str, incident: &Incident) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        if let Some(after) = tail.strip_prefix("{host}") {
            out.push_str(&incident.host);
            rest = after;
        } else if let Some(after) = tail.strip_prefix("{team}") {
            out.push_str(&incident.team);
            rest = after;
        } else if let Some(after) = tail.strip_prefix("{incident}") {
            out.push_str(&incident.incident);
            rest = after;
        } else {
            out.push('{');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// How one route ended, for [`Delivery::Failed`]: its exit status (or that
/// it did not run) plus a bounded snippet of what it printed.
fn failure_detail(route_index: usize, outcome: &CommandOutcome) -> String {
    let mut detail = match outcome.status {
        Some(status) => format!("route {route_index} exited {status}"),
        None => format!("route {route_index} did not run"),
    };
    let mut text = snippet(&outcome.stderr);
    if text.is_empty() {
        text = snippet(&outcome.stdout);
    }
    if !text.is_empty() {
        detail.push_str(": ");
        detail.push_str(&text);
    }
    detail
}

/// The first non-empty lines of `text`, bounded, as one line.
fn snippet(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(DETAIL_CHARS)
        .collect()
}

/// Seconds to wait before retrying an undelivered escalation, given how
/// many delivery attempts have already been made for this incident before
/// this retry (0 for the first retry wait): 60, then 300, then 900, capped
/// at 900.
#[must_use]
pub const fn backoff_seconds(attempts_before_retry: u8) -> u64 {
    match attempts_before_retry {
        0 => 60,
        1 => 300,
        _ => 900,
    }
}

/// Write `dir/alarm.json` (schema [`ALARM_SCHEMA`]) recording that no route
/// delivered `incident`, with the failure `detail` and the time `now` in
/// Unix seconds. The write is atomic (see `crate::agent::atomic_json`);
/// `dir` is created when missing, and its privacy stays the caller's.
pub fn write_alarm(dir: &Path, incident: &Incident, detail: &str, now: f64) -> AgentResult<()> {
    if !now.is_finite() {
        return Err(fail("alarm time must be finite"));
    }
    fs::create_dir_all(dir).map_err(|error| {
        fail(format!(
            "cannot create alarm directory {}: {error}",
            dir.display()
        ))
    })?;
    let value = json!({
        "schema": ALARM_SCHEMA,
        "host": incident.host,
        "team": incident.team,
        "incident": incident.incident,
        "text": incident.text,
        "detail": detail,
        "at": now,
    });
    crate::agent::atomic_json(&dir.join(ALARM_FILE), &value)
}

/// Remove `dir/alarm.json`; `true` when there was one. A missing alarm is
/// already clear, not an error.
pub fn clear_alarm(dir: &Path) -> AgentResult<bool> {
    let path = dir.join(ALARM_FILE);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(fail(format!("cannot remove {}: {error}", path.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    fn scratch() -> PathBuf {
        crate::turn_state::tests::scratch()
    }

    fn incident() -> Incident {
        Incident {
            host: "host-a".to_owned(),
            team: "team-a".to_owned(),
            incident: "incident-1".to_owned(),
            text: "the team needs a human\nsecond line".to_owned(),
        }
    }

    fn route(argv: &[&str], timeout_seconds: u64) -> Route {
        Route {
            argv: argv.iter().map(|element| (*element).to_owned()).collect(),
            timeout: Duration::from_secs(timeout_seconds),
        }
    }

    fn outcome(status: Option<i32>, stderr: &str) -> CommandOutcome {
        CommandOutcome {
            status,
            stdout: String::new(),
            stderr: stderr.to_owned(),
        }
    }

    /// One recorded [`Host::run_stdin`] call.
    #[derive(Clone, Debug, PartialEq)]
    struct Recorded {
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        timeout: Duration,
        stdin: String,
    }

    /// A scripted [`Host`]: `run_stdin` records and answers in script
    /// order; plain `run` fails closed, so a delivery that forgot stdin
    /// could never pass. No command ever executes.
    struct FakeHost {
        outcomes: RefCell<VecDeque<CommandOutcome>>,
        calls: RefCell<Vec<Recorded>>,
    }

    impl FakeHost {
        fn new(outcomes: Vec<CommandOutcome>) -> Self {
            Self {
                outcomes: RefCell::new(outcomes.into()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Recorded> {
            self.calls.borrow().clone()
        }
    }

    impl Host for FakeHost {
        fn boot_id(&self) -> AgentResult<String> {
            Ok("boot-fixture".to_owned())
        }

        fn boot_time(&self) -> Option<u64> {
            None
        }

        fn now(&self) -> f64 {
            1_000.0
        }

        fn sleep(&self, _duration: Duration) {}

        fn run(&self, _argv: &[String], _cwd: Option<&Path>, _timeout: Duration) -> CommandOutcome {
            outcome(None, "unexpected run without stdin")
        }

        fn run_stdin(
            &self,
            argv: &[String],
            cwd: Option<&Path>,
            timeout: Duration,
            stdin: &str,
        ) -> CommandOutcome {
            self.calls.borrow_mut().push(Recorded {
                argv: argv.to_vec(),
                cwd: cwd.map(Path::to_path_buf),
                timeout,
                stdin: stdin.to_owned(),
            });
            self.outcomes
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| outcome(None, "no scripted outcome"))
        }
    }

    #[test]
    fn a_valid_route_file_parses_with_defaults_escapes_and_comments() {
        let directory = scratch();
        let path = directory.join("escalation.toml");
        fs::write(
            &path,
            concat!(
                "# leading comment\n",
                "\n",
                "[[route]]\n",
                "argv = [\"/fixture/notify\", \"say \\\"hi\\\"\", \"a\\\\b\", \"tab\\tnl\\n\", \"#kept\"] # trailing comment\n",
                "timeout_seconds = 45\n",
                "\n",
                "[[route]] # the fallback\n",
                "argv=[\"/fixture/fallback\", \"{team}\"]\n",
            ),
        )
        .unwrap();
        assert_eq!(
            load_escalation_config(&path).unwrap(),
            EscalationConfig {
                routes: vec![
                    Route {
                        argv: vec![
                            "/fixture/notify".to_owned(),
                            "say \"hi\"".to_owned(),
                            "a\\b".to_owned(),
                            "tab\tnl\n".to_owned(),
                            "#kept".to_owned(),
                        ],
                        timeout: Duration::from_secs(45),
                    },
                    route(&["/fixture/fallback", "{team}"], 30),
                ],
            }
        );
        // A file of nothing but comments is a configured nothing: empty.
        fs::write(&path, "# no routes yet\n").unwrap();
        assert_eq!(
            load_escalation_config(&path).unwrap(),
            EscalationConfig::default()
        );
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_missing_route_file_is_empty_but_an_unreadable_one_is_an_error() {
        let directory = scratch();
        assert_eq!(
            load_escalation_config(&directory.join("absent.toml")).unwrap(),
            EscalationConfig::default()
        );
        let error = load_escalation_config(&directory).unwrap_err().to_string();
        assert!(error.contains("cannot read"), "{error}");
        assert!(error.contains(&directory.display().to_string()), "{error}");
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_parser_refuses_everything_outside_the_frozen_subset() {
        let cases: &[(&str, &str, usize)] = &[
            (
                "[[route]]\nargv = [\"/fixture/x\"]\ncolour = \"red\"\n",
                "unknown key \"colour\"",
                3,
            ),
            (
                "[[route]]\nargv = ['/fixture/x']\n",
                "one-line JSON array",
                2,
            ),
            (
                "[[route]]\nargv = [\"/fixture/x\", 3]\n",
                "one-line JSON array",
                2,
            ),
            (
                "[[route]]\nargv = \"/fixture/x\"\n",
                "one-line JSON array",
                2,
            ),
            ("[[route]]\nargv = []\n", "must name a program", 2),
            ("[[route]]\nargv = [\"\"]\n", "must name a program", 2),
            ("argv = [\"/fixture/x\"]\n", "outside a [[route]]", 1),
            ("[route]\nargv = [\"/fixture/x\"]\n", "unknown section", 1),
            ("[[route]]\ntimeout_seconds = 5\n", "has no argv", 1),
            (
                "[[route]]\nargv = [\"/fixture/x\"]\nargv = [\"/fixture/y\"]\n",
                "duplicate argv",
                3,
            ),
            (
                "[[route]]\nargv = [\"/fixture/x\"]\ntimeout_seconds = 0\n",
                "must be positive",
                3,
            ),
            (
                "[[route]]\nargv = [\"/fixture/x\"]\ntimeout_seconds = \"30\"\n",
                "positive integer",
                3,
            ),
            (
                "[[route]]\nargv [\"/fixture/x\"]\n",
                "expected KEY = VALUE",
                2,
            ),
            (
                "[[route]]\nargv = [\n\"/fixture/x\"]\n",
                "one-line JSON array",
                2,
            ),
        ];
        for (text, needle, line) in cases {
            let error = parse_escalation_config(text, Path::new("fixture.toml"))
                .unwrap_err()
                .to_string();
            assert!(
                error.starts_with(&format!("fixture.toml:{line}:")),
                "{error} (case {text:?})"
            );
            assert!(error.contains(needle), "{error} (case {text:?})");
        }
    }

    #[test]
    fn the_default_path_prefers_xdg_else_the_home_config() {
        let from = escalation_config_path_from;
        assert_eq!(
            from(Some(PathBuf::from("/cfg")), Some(PathBuf::from("/home-x"))),
            Some(PathBuf::from("/cfg/agent-utils/escalation.toml"))
        );
        // A relative or empty XDG_CONFIG_HOME names nothing (see the XDG
        // rule restore's report directory follows); the home answers.
        for xdg in [
            None,
            Some(PathBuf::new()),
            Some(PathBuf::from("relative/cfg")),
        ] {
            assert_eq!(
                from(xdg, Some(PathBuf::from("/home-x"))),
                Some(PathBuf::from("/home-x/.config/agent-utils/escalation.toml"))
            );
        }
        assert_eq!(from(None, None), None);
        assert_eq!(from(None, Some(PathBuf::new())), None);
    }

    #[test]
    fn the_first_route_fails_and_the_second_delivers_with_substitution_and_stdin() {
        let host = FakeHost::new(vec![
            outcome(Some(1), "nobody home"),
            outcome(Some(0), ""),
            outcome(Some(0), "must never be used"),
        ]);
        let config = EscalationConfig {
            routes: vec![
                route(&["/fixture/first", "{team}@{host}", "{incident}"], 5),
                route(&["/fixture/second", "page {team} {team}", "{unknown}"], 7),
                route(&["/fixture/third"], 9),
            ],
        };
        let incident = incident();
        assert_eq!(
            deliver(&host, &config, &incident),
            Delivery::Delivered { route_index: 1 }
        );
        assert_eq!(
            host.calls(),
            vec![
                Recorded {
                    argv: vec![
                        "/fixture/first".to_owned(),
                        "team-a@host-a".to_owned(),
                        "incident-1".to_owned(),
                    ],
                    cwd: None,
                    timeout: Duration::from_secs(5),
                    stdin: incident.text.clone(),
                },
                Recorded {
                    argv: vec![
                        "/fixture/second".to_owned(),
                        "page team-a team-a".to_owned(),
                        "{unknown}".to_owned(),
                    ],
                    cwd: None,
                    timeout: Duration::from_secs(7),
                    stdin: incident.text.clone(),
                },
            ],
            "routes after the delivering one are never tried"
        );
    }

    #[test]
    fn substitution_is_one_pass_and_leaves_other_braces_alone() {
        let incident = Incident {
            host: "h-{team}".to_owned(),
            ..incident()
        };
        assert_eq!(
            substitute("{host}", &incident),
            "h-{team}",
            "a substituted value is not rescanned"
        );
        assert_eq!(
            substitute("{} {hostile} {team}", &incident),
            "{} {hostile} team-a"
        );
        assert_eq!(substitute("no markers", &incident), "no markers");
    }

    #[test]
    fn every_route_failing_reports_each_routes_end() {
        let host = FakeHost::new(vec![
            outcome(Some(3), "nope\nmore detail\n\nignored tail line four"),
            outcome(None, "cannot run /fixture/gone"),
        ]);
        let config = EscalationConfig {
            routes: vec![route(&["/fixture/first"], 5), route(&["/fixture/gone"], 5)],
        };
        assert_eq!(
            deliver(&host, &config, &incident()),
            Delivery::Failed {
                detail: "route 0 exited 3: nope more detail ignored tail line four; \
                         route 1 did not run: cannot run /fixture/gone"
                    .to_owned(),
            }
        );
        assert_eq!(host.calls().len(), 2);
    }

    #[test]
    fn an_empty_argv_route_counts_as_failed_without_running() {
        let host = FakeHost::new(vec![outcome(Some(0), "")]);
        let config = EscalationConfig {
            routes: vec![Route::new(Vec::new()), route(&["/fixture/ok"], 5)],
        };
        assert_eq!(
            deliver(&host, &config, &incident()),
            Delivery::Delivered { route_index: 1 }
        );
        assert_eq!(host.calls().len(), 1, "only the runnable route ran");
        let host = FakeHost::new(Vec::new());
        let config = EscalationConfig {
            routes: vec![Route::new(Vec::new())],
        };
        assert_eq!(
            deliver(&host, &config, &incident()),
            Delivery::Failed {
                detail: "route 0: empty argv".to_owned(),
            }
        );
        assert!(host.calls().is_empty());
    }

    #[test]
    fn unconfigured_is_distinct_from_failed_and_runs_nothing() {
        let host = FakeHost::new(vec![outcome(Some(0), "")]);
        assert_eq!(
            deliver(&host, &EscalationConfig::default(), &incident()),
            Delivery::Unconfigured
        );
        assert!(host.calls().is_empty());
    }

    #[test]
    fn backoff_runs_sixty_three_hundred_then_caps_at_nine_hundred() {
        assert_eq!(backoff_seconds(0), 60);
        assert_eq!(backoff_seconds(1), 300);
        assert_eq!(backoff_seconds(2), 900);
        assert_eq!(backoff_seconds(3), 900);
        assert_eq!(backoff_seconds(u8::MAX), 900);
    }

    #[test]
    fn the_alarm_is_written_atomically_and_clears() {
        let directory = scratch();
        let state = directory.join("state");
        let incident = incident();
        write_alarm(&state, &incident, "route 0 exited 3: nope", 1_234.5).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(state.join(ALARM_FILE)).unwrap()).unwrap();
        assert_eq!(
            written,
            json!({
                "schema": "agentctl-team-alarm/v1",
                "host": "host-a",
                "team": "team-a",
                "incident": "incident-1",
                "text": "the team needs a human\nsecond line",
                "detail": "route 0 exited 3: nope",
                "at": 1_234.5,
            })
        );
        assert!(clear_alarm(&state).unwrap());
        assert!(!state.join(ALARM_FILE).exists());
        assert!(
            !clear_alarm(&state).unwrap(),
            "clearing twice is already clear"
        );
        assert!(write_alarm(&state, &incident, "again", f64::NAN).is_err());
        fs::remove_dir_all(&directory).unwrap();
    }
}
