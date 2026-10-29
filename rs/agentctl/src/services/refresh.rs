//! `services refresh`: move each service onto a snapshot, one at a time, with a backup, written
//! rollback commands, a health check, and an automatic rollback when the check fails.
//!
//! A transient unit (made with `systemd-run`) is replayed: every line of its definition becomes a
//! `systemd-run` property again, with only the agentctl binary (and, for a bridge, the
//! `AGENTCTL_HOME` plugin home) changed. Before the real unit is touched, the same properties
//! start a short-lived check unit that runs `sleep`, and its definition must read back as the
//! original's, so neither the change nor its rollback can fail on a property systemd-run cannot
//! express. A unit file gets a drop-in that replaces `ExecStart=` (and sets `AGENTCTL_HOME`),
//! followed by a daemon reload and a restart.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::discover::{normalize, BinarySite, Kind, Service};
use super::host::Host;
use super::snapshot::Snapshot;
use super::unit::{quote_word, shell_line, shell_quote, split_words, ExecCommand, UnitText};

/// File name of the drop-in written for a unit-file service.
pub const DROP_IN: &str = "zz-agentctl-services.conf";

/// `[Unit]` keys that name other units: a check unit leaves them out, so starting it never
/// starts, stops, or orders another unit.
const DEPENDENCY_KEYS: [&str; 15] = [
    "Wants",
    "Requires",
    "Requisite",
    "BindsTo",
    "PartOf",
    "Upholds",
    "Conflicts",
    "OnFailure",
    "OnSuccess",
    "After",
    "Before",
    "PropagatesReloadTo",
    "ReloadPropagatedFrom",
    "JoinsNamespaceOf",
    "PropagatesStopTo",
];

/// What `services refresh` was asked to do.
#[derive(Clone, Debug)]
pub struct RefreshOptions {
    /// The snapshot to move onto.
    pub snapshot: Snapshot,
    /// Only these units (empty: every service).
    pub only: Vec<String>,
    /// Report the plan without changing anything.
    pub dry_run: bool,
    /// Refresh a service that already runs this snapshot.
    pub force: bool,
    /// How long a restarted service has to pass its health check.
    pub health_timeout: Duration,
    /// How long to wait for a busy headless agent to finish its turn before skipping it.
    pub headless_wait: Duration,
    /// Where rollback material goes: one directory per run under it.
    pub refreshes_dir: PathBuf,
    /// Where drop-ins for unit-file services are written: `DIR/UNIT.d/zz-agentctl-services.conf`.
    pub drop_in_dir: PathBuf,
    /// Set by a signal handler: finish the service in hand, then stop.
    pub interrupted: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// How long a restarted service must stay up and healthy before it counts as refreshed.
pub const SOAK: Duration = Duration::from_secs(30);

/// How long systemd gets to unload a stopped transient unit before it is made again.
const UNLOAD_WAIT: Duration = Duration::from_secs(10);

/// How a service is moved.
#[derive(Clone, Debug)]
enum Method {
    /// Replay through `systemd-run`.
    Transient {
        forward: Vec<String>,
        rollback: Vec<String>,
        check: Vec<String>,
        expected: Definition,
        original: Definition,
    },
    /// Write a drop-in, reload, restart.
    DropIn {
        path: PathBuf,
        content: String,
        previous: Option<Vec<u8>>,
        expected_exec: Vec<String>,
    },
}

/// The intended change for one service.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The service as discovered.
    pub service: Service,
    /// The command it runs after the change.
    pub new_exec: ExecCommand,
    /// Environment changes: (name, old value, new value).
    pub environment: Vec<(String, Option<String>, String)>,
    method: Method,
}

/// The parts of a definition that must survive a replay, with resets applied.
pub type Definition = BTreeMap<(String, String), Vec<String>>;

/// The effective definition of parsed unit text: every key's values after resets, with
/// `Description` reduced to its last value and the generated header comments ignored.
pub fn definition(parsed: &UnitText, transient: bool) -> Result<Definition, String> {
    let mut keys: Vec<(String, String)> = Vec::new();
    for assignment in &parsed.assignments {
        let key = (assignment.section.clone(), assignment.key.clone());
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    let mut map = Definition::new();
    for (section, key) in keys {
        let mut values: Vec<String> = Vec::new();
        for value in parsed.effective(&section, &key) {
            let value = if transient {
                super::unit::unescape_transient(value)?
            } else {
                value.to_owned()
            };
            match key.as_str() {
                // Compare words, so equivalent quoting is equal.
                "ExecStart" => values.push(ExecCommand::parse(&value)?.render()),
                "Environment" => values.extend(split_words(&value)?),
                _ => values.push(value),
            }
        }
        if key == "Description" {
            values = values.into_iter().last().into_iter().collect();
        }
        if key == "Environment" {
            // A later assignment of a name replaces an earlier one.
            let mut seen: Vec<String> = Vec::new();
            let mut kept = Vec::new();
            for word in values.into_iter().rev() {
                let name = word.split_once('=').map_or(word.as_str(), |(name, _)| name);
                if !seen.iter().any(|existing| existing == name) {
                    seen.push(name.to_owned());
                    kept.push(word);
                }
            }
            kept.reverse();
            values = kept;
        }
        if !values.is_empty() {
            map.insert((section, key), values);
        }
    }
    Ok(map)
}

fn rewrite_exec(service: &Service, new_binary: &str) -> Result<ExecCommand, String> {
    let mut exec = service.exec.clone();
    match &service.site {
        BinarySite::Program => exec.argv[0] = new_binary.to_owned(),
        BinarySite::Script { script, start, end } => {
            let text = &mut exec.argv[*script];
            text.replace_range(*start..*end, &shell_quote(new_binary));
        }
    }
    Ok(exec)
}

/// Rewrite every `Environment=` line so `changes` take effect, appending a line for a name no
/// line sets. Returns (assignment index, new value) pairs in unit-file form; `usize::MAX` marks
/// an appended line.
fn rewrite_environment(
    parsed: &UnitText,
    changes: &[(String, Option<String>, String)],
) -> Result<Vec<(usize, String)>, String> {
    let mut rewritten = Vec::new();
    let mut set: Vec<String> = Vec::new();
    for (index, assignment) in parsed.assignments.iter().enumerate() {
        if assignment.section != "Service" || assignment.key != "Environment" {
            continue;
        }
        let mut words = split_words(&assignment.value)?;
        let mut changed = false;
        for word in &mut words {
            let Some(name) = word.split_once('=').map(|(name, _)| name.to_owned()) else {
                continue;
            };
            if let Some((_, _, new)) = changes.iter().find(|(change, _, _)| *change == name) {
                *word = format!("{name}={}", new.replace('%', "%%"));
                set.push(name);
                changed = true;
            }
        }
        if changed {
            let value = words
                .iter()
                .map(|word| quote_word(word))
                .collect::<Vec<_>>()
                .join(" ");
            rewritten.push((index, value));
        }
    }
    for (name, _, new) in changes {
        if !set.contains(name) {
            rewritten.push((
                usize::MAX,
                quote_word(&format!("{name}={}", new.replace('%', "%%"))),
            ));
        }
    }
    Ok(rewritten)
}

/// The `systemd-run` arguments that recreate `parsed` as transient unit `unit`, with `exec` as
/// its command and the given `Environment=` rewrites. `check` leaves out dependency keys.
fn systemd_run_args(
    unit: &str,
    parsed: &UnitText,
    exec: &[String],
    environment: &[(usize, String)],
    check: bool,
) -> Result<Vec<String>, String> {
    let mut args = vec![format!("--unit={unit}")];
    if let Some(description) = parsed.last("Unit", "Description") {
        args.push(format!(
            "--description={}",
            super::unit::unescape_transient(description)?
        ));
    }
    for (index, assignment) in parsed.assignments.iter().enumerate() {
        let key = assignment.key.as_str();
        match assignment.section.as_str() {
            "Unit" if key == "Description" => continue,
            "Unit" if check && DEPENDENCY_KEYS.contains(&key) => continue,
            "Unit" => {}
            "Service" if key == "ExecStart" => continue,
            "Service" if key.starts_with("Exec") => {
                return Err(format!(
                    "{key}= runs another command around the service; refresh it by hand"
                ))
            }
            "Service" => {}
            other => return Err(format!("unexpected [{other}] section in a transient unit")),
        }
        let value = match environment.iter().find(|(line, _)| *line == index) {
            Some((_, value)) => value.clone(),
            None => assignment.value.clone(),
        };
        args.push("-p".to_owned());
        args.push(format!(
            "{key}={}",
            super::unit::unescape_transient(&value)?
        ));
    }
    for (line, value) in environment {
        if *line == usize::MAX {
            args.push("-p".to_owned());
            args.push(format!(
                "Environment={}",
                super::unit::unescape_transient(value)?
            ));
        }
    }
    args.push("--".to_owned());
    args.extend(exec.iter().cloned());
    Ok(args)
}

/// Apply the intended edits to parsed text, for the definition the new unit must read back as.
fn edited(
    parsed: &UnitText,
    exec: &ExecCommand,
    environment: &[(usize, String)],
    transient: bool,
) -> UnitText {
    let mut text = parsed.clone();
    for (line, value) in environment {
        if *line == usize::MAX {
            text.assignments.push(super::unit::Assignment {
                section: "Service".into(),
                key: "Environment".into(),
                value: value.clone(),
            });
        } else {
            text.assignments[*line].value = value.clone();
        }
    }
    let mut rendered = exec.clone();
    if transient {
        for word in &mut rendered.argv {
            *word = word.replace('%', "%%");
        }
    }
    text.assignments.push(super::unit::Assignment {
        section: "Service".into(),
        key: "ExecStart".into(),
        value: String::new(),
    });
    text.assignments.push(super::unit::Assignment {
        section: "Service".into(),
        key: "ExecStart".into(),
        value: rendered.render(),
    });
    text
}

/// Plan moving `service` onto `snapshot`; `Ok(None)` when it already runs it.
pub fn plan(
    service: &Service,
    snapshot: &Snapshot,
    drop_in_dir: &Path,
    force: bool,
) -> Result<Option<Plan>, String> {
    let new_binary = snapshot.agentctl();
    let new_binary_text = new_binary
        .to_str()
        .ok_or("the snapshot path is not UTF-8")?
        .to_owned();
    let mut environment = Vec::new();
    if service.kind == Kind::Bridge {
        let home = snapshot.home().to_string_lossy().into_owned();
        let old = service
            .env(crate::plugins::AGENTCTL_HOME_ENV)
            .map(str::to_owned);
        if old.as_deref() != Some(home.as_str()) {
            environment.push((crate::plugins::AGENTCTL_HOME_ENV.to_owned(), old, home));
        }
    }
    let current = same_file(&service.binary, &new_binary) && environment.is_empty();
    if current && !force {
        return Ok(None);
    }
    if !service.binary_word.contains('/') {
        return Err(format!(
            "the unit names {} without a path, so the binary it runs cannot be checked",
            service.binary_word
        ));
    }
    if service.kind == Kind::Bridge {
        for key in ["EnvironmentFile", "UnsetEnvironment", "PassEnvironment"] {
            if !service.parsed.effective("Service", key).is_empty() {
                return Err(format!(
                    "{key}= may set or clear AGENTCTL_HOME behind Environment=; refresh it by hand"
                ));
            }
        }
        if let BinarySite::Script { script, start, .. } = &service.site {
            if service.exec.argv[*script][..*start].contains("AGENTCTL_HOME=") {
                return Err("its script sets AGENTCTL_HOME itself; refresh it by hand".to_owned());
            }
        }
    }
    let unit_binary = if service.state.transient {
        new_binary_text.clone()
    } else {
        new_binary_text.replace('%', "%%")
    };
    let new_exec = rewrite_exec(service, &unit_binary)?;
    let transient = service.state.transient;
    let method = if transient {
        if service.parsed.files.len() > 1 {
            return Err(format!(
                "the transient unit has drop-ins ({}), which a replay would apply twice; refresh it by hand",
                service.parsed.files[1..].join(", ")
            ));
        }
        if !service.exec.prefix.is_empty() {
            return Err(format!(
                "ExecStart= prefix {:?} cannot be replayed with systemd-run",
                service.exec.prefix
            ));
        }
        let rewrites = rewrite_environment(&service.parsed, &environment)?;
        let forward = systemd_run_args(
            &service.unit,
            &service.parsed,
            &new_exec.argv,
            &rewrites,
            false,
        )?;
        let rollback = systemd_run_args(
            &service.unit,
            &service.parsed,
            &service.exec.argv,
            &[],
            false,
        )?;
        let check_unit = check_unit_name(&service.unit);
        let mut check = systemd_run_args(&check_unit, &service.parsed, &[], &rewrites, true)?;
        check.pop(); // the "--" with no command after it
        if !matches!(
            service.parsed.last("Service", "Type"),
            None | Some("simple" | "exec")
        ) {
            check.push("-p".to_owned());
            check.push("Type=exec".to_owned());
        }
        // The check runs the planned command line as the arguments of a shell that only sleeps,
        // so systemd's quoting of every word is checked too, and nothing of the service starts.
        check.push("--".to_owned());
        check.extend(check_argv(&new_exec.argv));
        let expected = definition(&edited(&service.parsed, &new_exec, &rewrites, true), true)?;
        let original = definition(&service.parsed, true)?;
        Method::Transient {
            forward,
            rollback,
            check,
            expected,
            original,
        }
    } else {
        let path = drop_in_dir
            .join(format!("{}.d", service.unit))
            .join(DROP_IN);
        let previous = fs::read(&path).ok();
        let mut content = format!(
            "# Written by `agentctl services refresh` for snapshot {}.\n\
             # Remove this file, then run `systemctl --user daemon-reload` and\n\
             # `systemctl --user restart {}`, to run the unit's own command again.\n\
             [Service]\nExecStart=\nExecStart={}\n",
            snapshot.commit,
            service.unit,
            new_exec.render()
        );
        for (name, _, value) in &environment {
            content.push_str(&format!(
                "Environment={}\n",
                quote_word(&format!("{name}={}", value.replace('%', "%%")))
            ));
        }
        Method::DropIn {
            path,
            content,
            previous,
            expected_exec: new_exec.argv.clone(),
        }
    };
    Ok(Some(Plan {
        service: service.clone(),
        new_exec,
        environment,
        method,
    }))
}

/// The command a check unit runs: a shell that sleeps, given the intended command as arguments.
fn check_argv(argv: &[String]) -> Vec<String> {
    let mut words = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "exec /bin/sleep 300".to_owned(),
        "agentctl-services-check".to_owned(),
    ];
    words.extend(argv.iter().map(|word| word.replace("%%", "%")));
    words
}

/// The prefix every check unit's name starts with.
pub const CHECK_PREFIX: &str = "agentctl-services-check-";

fn check_unit_name(unit: &str) -> String {
    let stem = unit.strip_suffix(".service").unwrap_or(unit);
    format!("{CHECK_PREFIX}{}-{stem}.service", std::process::id())
}

fn unix_seconds(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// Whether two paths name the same file: canonically when both exist, else lexically.
pub fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => normalize(left) == normalize(right),
    }
}

fn strip_deleted(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_suffix(" (deleted)") {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

/// The health timeout for a service: a watcher gets at least its pass budget, a soak and a margin,
/// since its first pass may take that long.
fn health_timeout(service: &Service, options: &RefreshOptions) -> Duration {
    if service.kind != Kind::Watcher {
        return options.health_timeout;
    }
    let budget = option_value(&service.args, "pass-budget-seconds")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(120);
    options
        .health_timeout
        .max(Duration::from_secs(budget + 60) + SOAK)
}

/// Check once that a running service is healthy before anything changes: a refresh must not be
/// blamed for, or rolled back over, a problem the service already had.
fn preflight(host: &dyn Host, service: &Service) -> Result<String, String> {
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            host.sleep(Duration::from_secs(2));
        }
        let mut first_pid = None;
        match probe(
            host,
            service,
            &service.binary,
            &service.environment,
            UNIX_EPOCH,
            &mut first_pid,
        ) {
            Ok(detail) => return Ok(detail),
            Err(Fatal(detail) | Pending(detail)) => last = detail,
        }
    }
    Err(last)
}

/// Wait for `service` to run `binary` and pass the check for its kind, started at `started`.
pub fn health(
    host: &dyn Host,
    service: &Service,
    binary: &Path,
    environment: &[(String, String)],
    started: SystemTime,
    timeout: Duration,
) -> Result<String, String> {
    let deadline = started + timeout;
    let mut last;
    let mut first_pid: Option<u32> = None;
    loop {
        let outcome = probe(host, service, binary, environment, started, &mut first_pid);
        let now = host.now();
        match outcome {
            Ok(detail) if now.duration_since(started).unwrap_or_default() >= SOAK => {
                return Ok(detail)
            }
            Ok(detail) => last = format!("passing, waiting to see it stay up: {detail}"),
            Err(Fatal(detail)) => return Err(detail),
            Err(Pending(detail)) => last = detail,
        }
        if now >= deadline {
            return Err(format!(
                "no healthy state within {} s: {last}",
                timeout.as_secs()
            ));
        }
        host.sleep(Duration::from_secs(2));
    }
}

enum ProbeFailure {
    /// The service may still come up.
    Pending(String),
    /// The service will not come up as checked.
    Fatal(String),
}
use ProbeFailure::{Fatal, Pending};

fn probe(
    host: &dyn Host,
    service: &Service,
    binary: &Path,
    environment: &[(String, String)],
    started: SystemTime,
    first_pid: &mut Option<u32>,
) -> Result<String, ProbeFailure> {
    let state = host
        .show(std::slice::from_ref(&service.unit))
        .map_err(Pending)?
        .pop()
        .ok_or_else(|| Pending("systemctl show printed nothing".to_owned()))?;
    if state.active_state == "failed" {
        return Err(Fatal(format!("{} failed", service.unit)));
    }
    let Some(pid) = state.main_pid.filter(|_| state.active_state == "active") else {
        return Err(Pending(format!(
            "{} is {}",
            service.unit, state.active_state
        )));
    };
    match first_pid {
        None => *first_pid = Some(pid),
        Some(first) if *first != pid => {
            return Err(Fatal(format!(
                "its main process changed from {first} to {pid}: the service restarted"
            )))
        }
        Some(_) => {}
    }
    // A script without `exec` keeps the shell as the main process, with agentctl its child.
    let runs = |candidate: u32| {
        host.process_exe(candidate)
            .map(strip_deleted)
            .is_some_and(|exe| same_file(&exe, binary))
    };
    let pid = if runs(pid) {
        pid
    } else {
        host.process_children(pid)
            .into_iter()
            .find(|child| runs(*child))
            .ok_or_else(|| {
                Pending(format!(
                    "main process {pid} runs {} instead of {}",
                    host.process_exe(pid).map_or_else(
                        || "an unreadable executable".to_owned(),
                        |exe| exe.display().to_string()
                    ),
                    binary.display()
                ))
            })?
    };
    let since = host
        .now()
        .duration_since(started)
        .unwrap_or_default()
        .as_secs();
    match service.kind {
        Kind::Bridge => {
            let state_dir = service
                .bridge_state
                .as_ref()
                .ok_or_else(|| Fatal("no --bridge-state".into()))?;
            let mut args = vec![
                "--registry".to_owned(),
                service.registry.to_string_lossy().into_owned(),
            ];
            if let Some(herdr) = option_value(&service.args, "herdr-bin") {
                args.extend(["--herdr-bin".to_owned(), herdr]);
            }
            args.extend([
                "chat".to_owned(),
                "health".to_owned(),
                "--bridge-state".to_owned(),
                state_dir.to_string_lossy().into_owned(),
                "--json".to_owned(),
            ]);
            let (code, output) = host.run(binary, &args, environment).map_err(Pending)?;
            let report: Value = serde_json::from_str(&output)
                .map_err(|_| Pending(format!("chat health exited {code} without a JSON report")))?;
            let verdict = report["verdict"]
                .as_str()
                .unwrap_or("?")
                .to_ascii_uppercase();
            if code == 1 || verdict == "DOWN" {
                return Err(Pending(format!(
                    "chat health says DOWN: {}",
                    report["problems"]["down"]
                )));
            }
            if report["service"]["pid"].as_u64() != Some(u64::from(pid)) {
                return Err(Pending(format!(
                    "chat health sees service {} instead of process {pid}",
                    report["service"]["pid"]
                )));
            }
            // The bridge must have started the plugin home it was given.
            if let Some((_, home)) = environment
                .iter()
                .find(|(name, _)| name == crate::plugins::AGENTCTL_HOME_ENV)
            {
                let seen = host.process_environ(pid).and_then(|variables| {
                    variables
                        .into_iter()
                        .find(|(name, _)| name == crate::plugins::AGENTCTL_HOME_ENV)
                        .map(|(_, value)| value)
                });
                if seen.as_deref() != Some(home.as_str()) {
                    return Err(Fatal(format!(
                        "process {pid} runs with AGENTCTL_HOME {} instead of {home}",
                        seen.as_deref().unwrap_or("(unset or unreadable)")
                    )));
                }
            }
            // Both ages are whole seconds rounded down, so `<` proves the item came after the
            // restart, where `<=` would accept one from the second before it.
            match report["event_stream"]["last_item_age_seconds"].as_u64() {
                Some(age) if age < since => Ok(format!(
                    "chat health {verdict}, event stream {age} s old"
                )),
                Some(age) => Err(Pending(format!(
                    "chat health {verdict}, but the event stream's last item is {age} s old, from before the restart {since} s ago"
                ))),
                None => Err(Pending(format!("chat health {verdict}, event stream has delivered nothing yet"))),
            }
        }
        Kind::Watcher => {
            let directory = service
                .state_dir
                .as_ref()
                .ok_or_else(|| Fatal("no state directory".into()))?;
            let path = directory.join("agent-watch.json");
            let report: Value = host
                .read_file(&path)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .ok_or_else(|| Pending(format!("{} is not readable yet", path.display())))?;
            let checked = report["checked_at"].as_f64().unwrap_or(0.0);
            if checked >= unix_seconds(started) {
                Ok(format!(
                    "watch pass completed ({} agents)",
                    report["agents"]
                ))
            } else {
                Err(Pending(
                    "no watch pass has completed since the restart".to_owned(),
                ))
            }
        }
        Kind::Headless => {
            let directory = service
                .agent_directory()
                .ok_or_else(|| Fatal("no agent name".into()))?;
            let socket = crate::headless::control::socket_path(&directory);
            let status = host.headless_status(&socket).map_err(Pending)?;
            if status["ok"] != true {
                return Err(Pending(format!(
                    "the wrapper refused status: {}",
                    status["error"]
                )));
            }
            if status["wrapper_pid"].as_u64() != Some(u64::from(pid)) {
                return Err(Pending(format!(
                    "the control socket answers from process {} instead of {pid}",
                    status["wrapper_pid"]
                )));
            }
            Ok(format!(
                "wrapper answers ({})",
                status["status"].as_str().unwrap_or("?")
            ))
        }
    }
}

/// The value of a long option in agentctl arguments.
fn option_value(args: &[String], name: &str) -> Option<String> {
    let flag = format!("--{name}");
    let prefix = format!("--{name}=");
    let mut iter = args.iter();
    while let Some(word) = iter.next() {
        if *word == flag {
            return iter.next().cloned();
        }
        if let Some(value) = word.strip_prefix(&prefix) {
            return Some(value.to_owned());
        }
    }
    None
}

/// The environment a health check runs with: the unit's, with the intended changes.
fn check_environment(
    service: &Service,
    changes: &[(String, Option<String>, String)],
) -> Vec<(String, String)> {
    let mut environment = service.environment.clone();
    for (name, _, value) in changes {
        environment.retain(|(existing, _)| existing != name);
        environment.push((name.clone(), value.clone()));
    }
    environment
}

/// One service's line in the report.
fn row(plan_or_service: &Service, snapshot: &Snapshot, host: &dyn Host) -> Value {
    json!({
        "unit": plan_or_service.unit,
        "kind": plan_or_service.kind.label(),
        "old": {
            "binary": plan_or_service.binary,
            "commit": host.binary_commit(&plan_or_service.binary),
            "plugin_home": plan_or_service.plugin_home(),
        },
        "new": {
            "binary": snapshot.agentctl(),
            "commit": snapshot.commit,
            "plugin_home": (plan_or_service.kind == Kind::Bridge).then(|| snapshot.home()),
        },
    })
}

/// Where this run's rollback material goes.
pub struct RunDirectory {
    path: PathBuf,
}

impl RunDirectory {
    fn create(parent: &Path, snapshot: &Snapshot, now: SystemTime) -> Result<Self, String> {
        let seconds = now
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        let stamp = crate::chat_runtime::utc_timestamp(seconds * 1000).replace([':', '-'], "");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
        let path = parent.join(format!("{stamp}-{}", &snapshot.commit[..12]));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let directory = Self { path };
        directory.append(
            "ROLLBACK.sh",
            &format!(
                "#!/bin/sh\n# Rollback commands written by `agentctl services refresh` onto snapshot {}.\n\
                 # Each unit's commands were written before that unit was changed; run the blocks for\n\
                 # the units you want back, newest last. Each unit's definition before the change is\n\
                 # beside this file as UNIT.before.\n",
                snapshot.commit
            ),
        )?;
        let _ = fs::set_permissions(
            directory.path.join("ROLLBACK.sh"),
            fs::Permissions::from_mode(0o700),
        );
        Ok(directory)
    }

    fn append(&self, name: &str, text: &str) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt as _;
        let path = self.path.join(name);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.path.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Outcome of a whole refresh.
pub struct RefreshReport {
    /// The JSON report.
    pub document: Value,
    /// The text report.
    pub text: String,
    /// 0: every selected service runs the snapshot and is healthy (or was already current);
    /// 1: a service failed its change or check and was rolled back, and the rest were left
    /// alone; 4: a rollback failed too, so a service may be down.
    pub exit_code: i32,
}

/// Run `services refresh`.
pub fn refresh(
    host: &dyn Host,
    services: &[Service],
    options: &RefreshOptions,
) -> Result<RefreshReport, String> {
    let snapshot = &options.snapshot;
    let selected: Vec<&Service> = services
        .iter()
        .filter(|service| {
            options.only.is_empty()
                || options
                    .only
                    .iter()
                    .any(|only| service.unit == *only || service.unit == format!("{only}.service"))
        })
        .collect();
    for only in &options.only {
        if !selected
            .iter()
            .any(|service| service.unit == *only || service.unit == format!("{only}.service"))
        {
            return Err(format!(
                "--only {only}: no service of this registry has that unit name"
            ));
        }
    }
    // Watchers first and headless wrappers last: a failure stops the run before the services
    // whose restart costs most.
    let mut ordered = selected;
    ordered.sort_by_key(|service| match service.kind {
        Kind::Watcher => 0,
        Kind::Bridge => 1,
        Kind::Headless => 2,
    });
    let mut rows = Vec::new();
    let mut text = String::new();
    let mut exit_code = 0;
    let mut run: Option<RunDirectory> = None;
    let mut stopped = false;
    for service in ordered {
        let mut entry = row(service, snapshot, host);
        let label = format!("{} ({})", service.unit, service.kind.label());
        let old = format!(
            "{} {}",
            entry["old"]["commit"]
                .as_str()
                .map_or("unknown", |commit| &commit[..8]),
            service.binary.display()
        );
        let new = format!("{} {}", snapshot.short(), snapshot.agentctl().display());
        let interrupted = options
            .interrupted
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst));
        if stopped || interrupted {
            entry["action"] = json!("not-attempted");
            if interrupted && !stopped {
                exit_code = exit_code.max(1);
                text.push_str(&format!(
                    "{label}: not attempted: the refresh was interrupted\n"
                ));
            } else {
                text.push_str(&format!("{label}: not attempted after the failure above\n"));
            }
            stopped = true;
            rows.push(entry);
            continue;
        }
        if !matches!(service.state.active_state.as_str(), "active" | "reloading") {
            entry["action"] = json!("skipped");
            entry["reason"] = json!(format!(
                "the unit is {}, not active",
                service.state.active_state
            ));
            text.push_str(&format!(
                "{label}: skipped, the unit is {}\n",
                service.state.active_state
            ));
            rows.push(entry);
            continue;
        }
        let plan = match plan(service, snapshot, &options.drop_in_dir, options.force) {
            Ok(Some(plan)) => plan,
            Ok(None) => {
                entry["action"] = json!("current");
                text.push_str(&format!("{label}: already on {new}\n"));
                rows.push(entry);
                continue;
            }
            Err(reason) => {
                entry["action"] = json!("refused");
                entry["reason"] = json!(reason);
                text.push_str(&format!("{label}: refused: {reason}\n"));
                exit_code = exit_code.max(1);
                rows.push(entry);
                continue;
            }
        };
        entry["environment"] = json!(plan
            .environment
            .iter()
            .map(|(name, old, new)| json!({"name": name, "old": old, "new": new}))
            .collect::<Vec<_>>());
        entry["new_exec_start"] = json!(plan.new_exec.argv);
        if options.dry_run {
            entry["action"] = json!("would-refresh");
            entry["commands"] = json!(plan_commands(&plan));
            text.push_str(&format!("{label}: {old} -> {new} (dry run)\n"));
            for command in plan_commands(&plan) {
                text.push_str(&format!("    {command}\n"));
            }
            rows.push(entry);
            continue;
        }
        if service.kind == Kind::Headless {
            if let Some(reason) = wait_for_idle_headless(host, service, options.headless_wait) {
                entry["action"] = json!("skipped");
                entry["reason"] = json!(reason);
                text.push_str(&format!("{label}: skipped: {reason}\n"));
                rows.push(entry);
                continue;
            }
        }
        if let Err(reason) = preflight(host, service) {
            exit_code = exit_code.max(1);
            stopped = true;
            entry["action"] = json!("failed-unchanged");
            entry["error"] = json!(format!("not healthy before the refresh: {reason}"));
            text.push_str(&format!(
                "{label}: not healthy before the refresh, so it was left alone: {reason}\n"
            ));
            rows.push(entry);
            continue;
        }
        if run.is_none() {
            run = Some(RunDirectory::create(
                &options.refreshes_dir,
                snapshot,
                host.now(),
            )?);
        }
        let directory = run.as_ref().expect("created above");
        entry["rollback"] = json!(directory.path().join("ROLLBACK.sh"));
        match apply(host, &plan, options, directory) {
            Ok(detail) => {
                entry["action"] = json!("refreshed");
                entry["health"] = json!(detail);
                text.push_str(&format!("{label}: {old} -> {new}: healthy: {detail}\n"));
            }
            Err(Failure {
                error,
                rollback,
                changed,
            }) => {
                stopped = true;
                entry["error"] = json!(error);
                if !changed {
                    exit_code = exit_code.max(1);
                    entry["action"] = json!("failed-unchanged");
                    text.push_str(&format!(
                        "{label}: {old} -> {new} FAILED before any change: {error}\n"
                    ));
                    rows.push(entry);
                    continue;
                }
                match rollback {
                    Ok(detail) => {
                        exit_code = exit_code.max(1);
                        entry["action"] = json!("rolled-back");
                        entry["rollback_health"] = json!(detail);
                        text.push_str(&format!(
                            "{label}: {old} -> {new} FAILED: {error}\n    rolled back to {old}: {detail}\n"
                        ));
                    }
                    Err(rollback_error) => {
                        exit_code = 4;
                        entry["action"] = json!("rollback-failed");
                        entry["rollback_error"] = json!(rollback_error);
                        text.push_str(&format!(
                            "{label}: {old} -> {new} FAILED: {error}\n    ROLLBACK FAILED: {rollback_error}\n    run the commands in {} by hand\n",
                            directory.path().join("ROLLBACK.sh").display()
                        ));
                    }
                }
            }
        }
        rows.push(entry);
    }
    if rows.is_empty() {
        text.push_str("no services of this registry to refresh\n");
    }
    let document = json!({
        "schema": "agentctl-services-refresh/v1",
        "snapshot": {"commit": snapshot.commit, "path": snapshot.path},
        "dry_run": options.dry_run,
        "run_directory": run.as_ref().map(RunDirectory::path),
        "services": rows,
        "exit_code": exit_code,
    });
    if let Some(directory) = &run {
        // Services may have changed already: a report that cannot be saved is a warning, never a
        // reason to lose the outcome.
        let saved = serde_json::to_vec_pretty(&document)
            .map_err(|error| error.to_string())
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                directory.write("report.json", &bytes)
            });
        if let Err(error) = saved {
            text.push_str(&format!("warning: report.json was not written: {error}\n"));
        }
        text.push_str(&format!(
            "rollback material: {}\n",
            directory.path().display()
        ));
    }
    Ok(RefreshReport {
        document,
        text,
        exit_code,
    })
}

/// `None` once the headless agent is idle, else why it is skipped.
fn wait_for_idle_headless(host: &dyn Host, service: &Service, wait: Duration) -> Option<String> {
    let socket = crate::headless::control::socket_path(&service.agent_directory()?);
    let deadline = host.now() + wait;
    loop {
        match host.headless_status(&socket) {
            Ok(status) if status["busy"] != true => return None,
            Ok(_) if host.now() >= deadline => {
                return Some(format!(
                    "the agent is still in a turn after {} s; run refresh again when it is idle",
                    wait.as_secs()
                ))
            }
            Ok(_) => host.sleep(Duration::from_secs(5)),
            // A wrapper that does not answer has no turn to lose; the health check judges it.
            Err(_) => return None,
        }
    }
}

/// The shell commands a plan runs, for a dry run.
fn plan_commands(plan: &Plan) -> Vec<String> {
    let unit = &plan.service.unit;
    match &plan.method {
        Method::Transient { forward, check, .. } => vec![
            format!("systemd-run --user {}", shell_line(check)),
            format!(
                "systemctl --user stop {}",
                shell_quote(&check_unit_name(unit))
            ),
            format!("systemctl --user stop {}", shell_quote(unit)),
            format!("systemctl --user reset-failed {}", shell_quote(unit)),
            format!("systemd-run --user {}", shell_line(forward)),
        ],
        Method::DropIn { path, content, .. } => vec![
            format!(
                "write {} with ExecStart={}",
                path.display(),
                content
                    .lines()
                    .filter_map(|line| line.strip_prefix("ExecStart="))
                    .find(|value| !value.is_empty())
                    .unwrap_or("")
            ),
            "systemctl --user daemon-reload".to_owned(),
            format!("systemctl --user restart {}", shell_quote(unit)),
        ],
    }
}

struct Failure {
    error: String,
    /// `Ok` with how the old service checks after the rollback; `Err` when the rollback failed.
    rollback: Result<String, String>,
    /// Whether the unit was touched before the failure.
    changed: bool,
}

fn rollback_block(plan: &Plan, directory: &RunDirectory) -> Result<String, String> {
    let unit = &plan.service.unit;
    let mut block = format!("\n# {unit}: back to {}\n", plan.service.binary.display());
    match &plan.method {
        Method::Transient { rollback, .. } => {
            block.push_str(&format!(
                "systemctl --user stop {unit}\nsystemctl --user reset-failed {unit} || true\nsystemd-run --user {}\n",
                shell_line(rollback),
                unit = shell_quote(unit)
            ));
        }
        Method::DropIn { path, previous, .. } => {
            match previous {
                Some(bytes) => {
                    let saved = format!("{unit}.previous-drop-in.conf");
                    directory.write(&saved, bytes)?;
                    block.push_str(&format!(
                        "cp {} {}\n",
                        shell_quote(&directory.path().join(saved).to_string_lossy()),
                        shell_quote(&path.to_string_lossy())
                    ));
                }
                None => {
                    block.push_str(&format!("rm -f {}\n", shell_quote(&path.to_string_lossy())))
                }
            }
            block.push_str(&format!(
                "systemctl --user daemon-reload\nsystemctl --user restart {}\n",
                shell_quote(unit)
            ));
        }
    }
    Ok(block)
}

/// Change one service, check it, and roll it back on failure.
fn apply(
    host: &dyn Host,
    plan: &Plan,
    options: &RefreshOptions,
    directory: &RunDirectory,
) -> Result<String, Failure> {
    let early = |error: String| Failure {
        error,
        rollback: Ok("nothing was changed".to_owned()),
        changed: false,
    };
    let service = &plan.service;
    // The plan was made from the definition read at discovery; a unit changed since is refused.
    match host.cat(&service.unit) {
        Ok(text) if text == service.text => {}
        Ok(_) => {
            return Err(early(
                "the unit's definition changed since it was read; run refresh again".to_owned(),
            ))
        }
        Err(error) => return Err(early(error)),
    }
    // Back up and write the rollback commands just before the first change.
    let record = || -> Result<(), String> {
        directory.write(&format!("{}.before", service.unit), service.text.as_bytes())?;
        directory.append("ROLLBACK.sh", &rollback_block(plan, directory)?)
    };
    let timeout = health_timeout(service, options);
    let new_environment = check_environment(service, &plan.environment);
    let old_environment = service.environment.clone();
    match &plan.method {
        Method::Transient {
            forward,
            rollback,
            check,
            expected,
            original,
        } => {
            check_replay(host, &service.unit, check, expected, &new_exec_argv(plan))
                .map_err(early)?;
            record().map_err(early)?;
            host.stop(&service.unit).map_err(|error| Failure {
                rollback: restore_transient(
                    host,
                    service,
                    rollback,
                    original,
                    &old_environment,
                    timeout,
                ),
                error: format!("stopping the old unit failed: {error}"),
                changed: true,
            })?;
            host.reset_failed(&service.unit);
            let started = host.now();
            let outcome = wait_unloaded(host, &service.unit)
                .and_then(|()| host.systemd_run(forward))
                .and_then(|()| {
                    let text = host.cat(&service.unit)?;
                    let actual = definition(&UnitText::parse(&text), true)?;
                    if actual == *expected {
                        Ok(())
                    } else {
                        Err(format!(
                            "the new unit's definition differs from the planned one: {}",
                            difference(expected, &actual)
                        ))
                    }
                })
                .and_then(|()| {
                    health(
                        host,
                        service,
                        &options.snapshot.agentctl(),
                        &new_environment,
                        started,
                        timeout,
                    )
                });
            outcome.map_err(|error| Failure {
                rollback: restore_transient(
                    host,
                    service,
                    rollback,
                    original,
                    &old_environment,
                    timeout,
                ),
                error,
                changed: true,
            })
        }
        Method::DropIn {
            path,
            content,
            previous,
            expected_exec,
        } => {
            let write = || -> Result<(), String> {
                let parent = path.parent().ok_or("drop-in path has no directory")?;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o755)
                    .create(parent)
                    .map_err(|error| format!("{}: {error}", parent.display()))?;
                write_atomically(path, content.as_bytes())
            };
            record().map_err(early)?;
            write().map_err(|error| Failure {
                rollback: restore_drop_in(
                    host,
                    service,
                    path,
                    previous.as_deref(),
                    &old_environment,
                    timeout,
                ),
                error,
                changed: true,
            })?;
            let restore = || {
                restore_drop_in(
                    host,
                    service,
                    path,
                    previous.as_deref(),
                    &old_environment,
                    timeout,
                )
            };
            let started = host.now();
            let outcome = host
                .daemon_reload()
                .and_then(|()| {
                    let text = host.cat(&service.unit)?;
                    let parsed = UnitText::parse(&text);
                    let commands = parsed.effective("Service", "ExecStart");
                    let exec = commands
                        .last()
                        .map(|value| ExecCommand::parse(value))
                        .transpose()?;
                    if commands.len() == 1 && exec.as_ref().map(|exec| &exec.argv) == Some(expected_exec) {
                        Ok(())
                    } else {
                        Err(format!(
                            "after the reload the unit's ExecStart is not the planned one (is another drop-in sorted after {DROP_IN}?)"
                        ))
                    }
                })
                .and_then(|()| host.restart(&service.unit))
                .and_then(|()| {
                    health(
                        host,
                        service,
                        &options.snapshot.agentctl(),
                        &new_environment,
                        started,
                        timeout,
                    )
                });
            outcome.map_err(|error| Failure {
                rollback: restore(),
                error,
                changed: true,
            })
        }
    }
}

/// The intended command of a plan, as the words a manager runs.
fn new_exec_argv(plan: &Plan) -> Vec<String> {
    plan.new_exec.argv.clone()
}

/// Prove that `check` (the intended properties, with the intended command handed to a shell that
/// only sleeps) is accepted and reads back as `expected` apart from dependency keys, then remove
/// the check unit.
fn check_replay(
    host: &dyn Host,
    unit: &str,
    check: &[String],
    expected: &Definition,
    argv: &[String],
) -> Result<(), String> {
    let check_unit = check_unit_name(unit);
    host.systemd_run(check)
        .map_err(|error| format!("systemd-run refused the unit's properties: {error}"))?;
    let read = host.cat(&check_unit);
    let stopped = host.stop(&check_unit);
    host.reset_failed(&check_unit);
    stopped.map_err(|error| format!("the check unit {check_unit} did not stop: {error}"))?;
    let actual = definition(&UnitText::parse(&read?), true)?;
    let type_overridden = check.iter().any(|word| word == "Type=exec");
    let comparable = |definition: &Definition| -> Definition {
        definition
            .iter()
            .filter(|((section, key), _)| {
                !(section == "Unit" && DEPENDENCY_KEYS.contains(&key.as_str()))
                    && !(type_overridden && key == "Type")
            })
            .map(|(key, values)| (key.clone(), values.clone()))
            .collect()
    };
    let mut want = comparable(expected);
    want.insert(
        ("Service".to_owned(), "ExecStart".to_owned()),
        vec![ExecCommand {
            prefix: String::new(),
            argv: check_argv(argv),
        }
        .render()],
    );
    let got = comparable(&actual);
    if want == got {
        Ok(())
    } else {
        Err(format!(
            "systemd-run does not reproduce the unit's definition: {}",
            difference(&want, &got)
        ))
    }
}

/// Wait for systemd to unload a stopped transient unit, so its name is free for the new one.
fn wait_unloaded(host: &dyn Host, unit: &str) -> Result<(), String> {
    let deadline = host.now() + UNLOAD_WAIT;
    loop {
        let state = host.show(&[unit.to_owned()])?.pop();
        if state.is_none_or(|state| state.load_state == "not-found") {
            return Ok(());
        }
        if host.now() >= deadline {
            return Err(format!(
                "{unit} stays loaded after it stopped (another unit or a client still refers to it)"
            ));
        }
        host.sleep(Duration::from_millis(500));
    }
}

/// Stale check units: ones whose refresh process is gone.
pub fn reap_check_units(host: &dyn Host) {
    let Ok(units) = host.service_units() else {
        return;
    };
    for unit in units {
        let Some(rest) = unit.strip_prefix(CHECK_PREFIX) else {
            continue;
        };
        let pid = rest
            .split('-')
            .next()
            .and_then(|pid| pid.parse::<u32>().ok());
        if pid.is_some_and(|pid| pid != std::process::id() && host.process_exe(pid).is_none()) {
            let _ = host.stop(&unit);
            host.reset_failed(&unit);
        }
    }
}

fn difference(want: &Definition, got: &Definition) -> String {
    let mut parts = Vec::new();
    for (key, values) in want {
        if got.get(key) != Some(values) {
            parts.push(format!(
                "{}.{}: want {:?}, got {:?}",
                key.0,
                key.1,
                values,
                got.get(key)
            ));
        }
    }
    for (key, values) in got {
        if !want.contains_key(key) {
            parts.push(format!("{}.{}: unexpected {:?}", key.0, key.1, values));
        }
    }
    parts.join("; ")
}

fn restore_transient(
    host: &dyn Host,
    service: &Service,
    rollback: &[String],
    original: &Definition,
    environment: &[(String, String)],
    timeout: Duration,
) -> Result<String, String> {
    let _ = host.stop(&service.unit);
    host.reset_failed(&service.unit);
    let started = host.now();
    if wait_unloaded(host, &service.unit).is_ok() {
        host.systemd_run(rollback)?;
    } else {
        // The unit could not be unloaded (or never stopped): when it still holds the original
        // definition, starting it again is the rollback.
        let loaded = definition(&UnitText::parse(&host.cat(&service.unit)?), true)?;
        if loaded != *original {
            return Err(format!(
                "{} stays loaded with a definition that is not the backup: {}",
                service.unit,
                difference(original, &loaded)
            ));
        }
        host.start(&service.unit)?;
    }
    let actual = definition(&UnitText::parse(&host.cat(&service.unit)?), true)?;
    if actual != *original {
        return Err(format!(
            "the restored unit's definition differs from the backup: {}",
            difference(original, &actual)
        ));
    }
    health(
        host,
        service,
        &service.binary,
        environment,
        started,
        timeout,
    )
    .map_err(|error| format!("restored to the old definition and binary, but {error}"))
}

fn restore_drop_in(
    host: &dyn Host,
    service: &Service,
    path: &Path,
    previous: Option<&[u8]>,
    environment: &[(String, String)],
    timeout: Duration,
) -> Result<String, String> {
    match previous {
        Some(bytes) => write_atomically(path, bytes)?,
        None => match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", path.display())),
        },
    }
    host.daemon_reload()?;
    let started = host.now();
    host.restart(&service.unit)?;
    health(
        host,
        service,
        &service.binary,
        environment,
        started,
        timeout,
    )
    .map_err(|error| format!("restored to the old definition and binary, but {error}"))
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("{}: {error}", path.display())
    })
}
