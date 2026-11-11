//! `agentctl services turnover`: turn every daemon on the host
//! over to main, only where the code it runs from changed (#1179).
//!
//! Discovery is host-wide: every user unit running an agentctl binary,
//! whatever registry it names, plus other daemons of this codebase found by
//! their binary (reported; the snapshot carries no binary for them).
//! Scope is opt-out: only a documented pin skips a unit or a registry
//! (`REGISTRY/turnover.pin`, `REGISTRY/turnover-pins/<unit>.pin`, reason =
//! file contents). Refresh of agentctl units reuses `services refresh`
//! per registry, bridges last within a registry, and the global agentctl
//! binary moves last of all under the same changed-paths test.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use super::discover::{self, Kind, OptionTables, Service};
use super::host::Host;
use super::refresh::RefreshOptions;
use super::snapshot::{self, BuildOptions, Snapshot};

/// Binaries that mark a unit as a daemon of this codebase the snapshot cannot
/// move (it carries only agentctl and its Slack plugin): discovered and
/// reported, never refreshed by turnover.
const OTHER_BINARIES: [&str; 6] = [
    "tick-hub",
    "wrkslots",
    "wrkslotsd",
    "dagrun",
    "agent-teams",
    "wrkviz",
];

/// What a turnover run was asked to do.
pub struct TurnoverOptions {
    /// Where snapshots and refresh records live.
    pub root: PathBuf,
    /// Where drop-ins for unit-file services go.
    pub drop_in_dir: PathBuf,
    /// The Git checkout main is resolved and built from.
    pub checkout: PathBuf,
    /// The revision to turn over to (default HEAD in the CLI).
    pub revision: String,
    /// The cargo executable for the snapshot build.
    pub cargo: PathBuf,
    /// Parent of the snapshot build's temporary directories.
    pub work_dir: PathBuf,
    /// Keep the build's temporary directory.
    pub keep_work: bool,
    /// Read-only: print decisions, build and move nothing.
    pub report_only: bool,
    /// Print the actions in order; change nothing.
    pub dry_run: bool,
    /// Also print the JSON document fields the text report carries.
    pub health_timeout: Duration,
    /// Headless turn wait, as in refresh.
    pub headless_wait: Duration,
    /// Notify command (flag form; a registry's own setting wins per registry).
    pub notify: Option<Vec<String>>,
    /// The global agentctl binary to move last; `None` skips it.
    pub global_binary: Option<PathBuf>,
    /// Signal flag passed through to each refresh.
    pub interrupted: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// The designed notify identity for timer-driven turnover runs (#1220):
/// a valid agent name the sender header can name and be answered by, set
/// as `AGENTCTL_AGENT` by the scheduled entry and by the notify wrapper.
pub const TIMER_NOTIFY_AGENT: &str = "turnover-timer";

/// How a scheduled (timer) service treats a turnover exit code (#1220).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduledOutcome {
    /// The run completed: 0 (all current or refreshed) or 74 (report:
    /// some behind or report-only, nothing failed).
    Success,
    /// The run failed and the coordinator is alerted: 1 (a refresh failed
    /// and rolled back), 4 (a rollback failed), 75 (nothing could move,
    /// or a report had errors or unknowns), and any other code.
    Failure,
}

/// Map every turnover exit code to its scheduled-service outcome.
#[must_use]
pub fn scheduled_outcome(exit_code: i32) -> ScheduledOutcome {
    match exit_code {
        0 | 74 => ScheduledOutcome::Success,
        _ => ScheduledOutcome::Failure,
    }
}

/// The file-based notify message for one TURNOVER_* environment (#1220):
/// phase, registry, unit, both commits and the verdict, one per line.
#[must_use]
pub fn notify_message(get: impl Fn(&str) -> Option<String>) -> String {
    let value = |name: &str| get(name).unwrap_or_default();
    format!(
        "turnover {phase}: registry {registry}, unit {unit}, {old} -> {new}, verdict {verdict}\n",
        phase = value("TURNOVER_PHASE"),
        registry = value("TURNOVER_REGISTRY"),
        unit = value("TURNOVER_UNIT"),
        old = value("TURNOVER_OLD_SHA"),
        new = value("TURNOVER_NEW_SHA"),
        verdict = value("TURNOVER_VERDICT"),
    )
}

/// Run as the timer's `--notify` command (`services turnover-notify`):
/// format the TURNOVER_* environment into a file and send it with
/// `agentctl send TO --file FILE` under the timer notify identity.
/// Returns the send's exit code.
pub fn run_turnover_notify(to: &str, agent: &str) -> Result<i32, String> {
    let message = notify_message(|name| std::env::var(name).ok());
    let file = std::env::temp_dir().join(format!(
        "agentctl-turnover-notify-{}.txt",
        std::process::id()
    ));
    write_private(&file, message.as_bytes(), 0o600)?;
    let agentctl = std::env::current_exe().map_err(|error| error.to_string())?;
    let status = std::process::Command::new(&agentctl)
        .env("AGENTCTL_AGENT", agent)
        .arg("send")
        .arg(to)
        .arg("--file")
        .arg(&file)
        .status()
        .map_err(|error| format!("cannot run {} send: {error}", agentctl.display()));
    let _ = fs::remove_file(&file);
    match status {
        Ok(status) => Ok(status.code().unwrap_or(1)),
        Err(error) => Err(error),
    }
}

/// One unit's turnover decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Decision {
    Refresh,
    Unchanged,
    Pinned,
    Ahead,
    Unknown,
    ReportOnly,
}

impl Decision {
    fn label(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Unchanged => "unchanged",
            Self::Pinned => "pinned",
            Self::Ahead => "ahead",
            Self::Unknown => "unknown",
            Self::ReportOnly => "report-only",
        }
    }
}

/// One report row, agentctl service or other daemon.
struct Row {
    kind_label: String,
    registry: Option<PathBuf>,
    unit: String,
    binary: PathBuf,
    running: Option<String>,
    changed_paths: Vec<String>,
    decision: Decision,
    detail: String,
    service: Option<Service>,
}

fn sha8(commit: Option<&str>) -> String {
    commit
        .map(|commit| commit.get(..8).unwrap_or(commit).to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Paths whose change can change what a service runs, per kind.
fn affecting_paths(kind: Kind) -> Vec<&'static str> {
    let mut paths = vec![
        "rs/agentctl",
        "rs/agent-queue",
        "rs/agent-harness",
        "rs/term-backend",
        "rs/Cargo.toml",
        "rs/Cargo.lock",
    ];
    if kind == Kind::Bridge {
        paths.extend([
            "rs/chat-subscription",
            "rs/chat-subscription-plugin",
            "rs/chat-subscription-slack",
        ]);
    }
    paths
}

/// Paths affecting a non-agentctl binary of this codebase, by name.
fn other_affecting_paths(binary: &str) -> Vec<String> {
    vec![
        format!("rs/{binary}"),
        "rs/Cargo.toml".to_owned(),
        "rs/Cargo.lock".to_owned(),
    ]
}

fn git(checkout: &Path, args: &[String]) -> Result<(i32, String), String> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match snapshot::git_in(checkout, &refs) {
        Ok(output) => Ok((0, output)),
        Err(_) => Ok((1, String::new())),
    }
}

/// The changed paths in `paths` between two commits, and whether the
/// running commit is ahead of the candidate. `None` when either commit
/// cannot be compared in this checkout.
fn change_test(
    checkout: &Path,
    running: &str,
    candidate: &str,
    paths: &[String],
) -> Option<(Vec<String>, bool)> {
    if running == candidate {
        return Some((Vec::new(), false));
    }
    let mut args = vec![
        "diff".to_owned(),
        "--name-only".to_owned(),
        format!("{running}..{candidate}"),
        "--".to_owned(),
    ];
    args.extend(paths.iter().cloned());
    let (code, output) = git(checkout, &args).ok()?;
    if code != 0 {
        return None;
    }
    let changed: Vec<String> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect();
    let ahead = git(
        checkout,
        &[
            "merge-base".to_owned(),
            "--is-ancestor".to_owned(),
            candidate.to_owned(),
            running.to_owned(),
        ],
    )
    .is_ok_and(|(code, _)| code == 0);
    Some((changed, ahead))
}

/// A registry's pins: whole-registry reason, and per-unit reasons by unit name.
struct Pins {
    registry: Option<String>,
    units: BTreeMap<String, String>,
}

fn pin_reason(host: &dyn Host, path: &Path) -> Result<Option<String>, String> {
    let Some(bytes) = host.read_file(path) else {
        return Ok(None);
    };
    let reason = String::from_utf8_lossy(&bytes).trim().to_owned();
    if reason.is_empty() {
        return Err(format!(
            "pin {} names no reason; write why it is pinned or remove it",
            path.display()
        ));
    }
    Ok(Some(reason))
}

fn read_pins(host: &dyn Host, registry: &Path, units: &[&str]) -> Result<Pins, String> {
    let mut pins = Pins {
        registry: pin_reason(host, &registry.join("turnover.pin"))?,
        units: BTreeMap::new(),
    };
    for unit in units {
        if let Some(reason) = pin_reason(
            host,
            &registry.join("turnover-pins").join(format!("{unit}.pin")),
        )? {
            pins.units.insert((*unit).to_owned(), reason);
        }
    }
    Ok(pins)
}

/// #1220: a pin directory that stays in the registry is private, and one
/// whose last pin file was removed is removed, so a staged run leaves no
/// empty pin directory behind for the agent scan to misread.
fn tidy_pin_dir(registry: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = registry.join("turnover-pins");
    let empty = fs::read_dir(&dir).is_ok_and(|mut entries| entries.next().is_none());
    if empty {
        let _ = fs::remove_dir(&dir);
    } else if dir.is_dir() {
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
}

impl Pins {
    fn reason_for(&self, unit: &str) -> Option<&str> {
        self.units
            .get(unit)
            .map(String::as_str)
            .or(self.registry.as_deref())
    }
}

/// Units running other binaries of this codebase: discovered from their
/// `ExecStart=` first word, reported with a remedy, never refreshed (the
/// snapshot carries no binary for them).
fn other_units(host: &dyn Host) -> Vec<(String, PathBuf)> {
    let Ok(units) = host.service_units() else {
        return Vec::new();
    };
    let Ok(states) = host.show(&units) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for state in states {
        // The ExecStart property names the program after `path=`.
        let Some(program) = state
            .exec_start
            .split_whitespace()
            .find_map(|word| word.strip_prefix("path="))
        else {
            continue;
        };
        let name = program.rsplit('/').next().unwrap_or(program);
        if OTHER_BINARIES.contains(&name) {
            found.push((state.id, PathBuf::from(program)));
        }
    }
    found
}

/// The notify command for one registry: its own `turnover-notify` file
/// (first non-empty, non-comment line) wins over the run's `--notify`.
fn notify_command(
    host: &dyn Host,
    registry: &Path,
    flag: Option<&[String]>,
) -> Option<Vec<String>> {
    if let Some(bytes) = host.read_file(&registry.join("turnover-notify")) {
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Ok(words) = super::unit::shell_words(line) {
                let argv: Vec<String> = words
                    .into_iter()
                    .take_while(|word| !word.operator)
                    .map(|word| word.text)
                    .collect();
                if !argv.is_empty() {
                    return Some(argv);
                }
            }
            return None;
        }
        return None;
    }
    flag.map(<[String]>::to_vec)
}

#[allow(clippy::too_many_arguments)]
fn notify(
    host: &dyn Host,
    command: Option<&[String]>,
    phase: &str,
    registry: &Path,
    unit: &str,
    old_sha: Option<&str>,
    new_sha: &str,
    verdict: &str,
    notes: &mut Vec<String>,
) {
    let Some(command) = command.filter(|command| !command.is_empty()) else {
        return;
    };
    let environment = vec![
        ("TURNOVER_PHASE".to_owned(), phase.to_owned()),
        (
            "TURNOVER_REGISTRY".to_owned(),
            registry.display().to_string(),
        ),
        ("TURNOVER_UNIT".to_owned(), unit.to_owned()),
        (
            "TURNOVER_OLD_SHA".to_owned(),
            old_sha.unwrap_or("").to_owned(),
        ),
        ("TURNOVER_NEW_SHA".to_owned(), new_sha.to_owned()),
        ("TURNOVER_VERDICT".to_owned(), verdict.to_owned()),
    ];
    match host.run(Path::new(&command[0]), &command[1..], &environment) {
        Ok((0, _)) => {}
        Ok((code, _)) => notes.push(format!(
            "notify for {} unit {unit} exited {code}",
            registry.display()
        )),
        Err(error) => notes.push(format!(
            "notify for {} unit {unit} could not run: {error}",
            registry.display()
        )),
    }
}

/// Move the global binary onto the snapshot, keeping a backup the rollback
/// restores when the new binary does not report the candidate commit.
fn move_global_binary(
    host: &dyn Host,
    options: &TurnoverOptions,
    snapshot: &Snapshot,
    candidate: &str,
) -> Result<String, String> {
    let Some(global) = options.global_binary.as_deref() else {
        return Ok("skipped (no global binary configured)".to_owned());
    };
    let source = snapshot.agentctl();
    if super::refresh::same_file(global, &source) {
        return Ok("already the snapshot binary".to_owned());
    }
    let backups = options.root.join("global-backups");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&backups)
        .map_err(|error| format!("{}: {error}", backups.display()))?;
    let old_commit = host.binary_commit(global);
    let backup = backups.join(format!(
        "agentctl-{}",
        old_commit
            .as_deref()
            .map_or("unknown", |commit| &commit[..8])
    ));
    let link_target = fs::read_link(global).ok();
    if link_target.is_none() {
        let bytes = fs::read(global).map_err(|error| format!("{}: {error}", global.display()))?;
        write_private(&backup, &bytes, 0o755)?;
    }
    let moved = (|| -> Result<(), String> {
        if link_target.is_some() {
            let temporary = global.with_extension(format!("turnover.{}.tmp", std::process::id()));
            let _ = fs::remove_file(&temporary);
            std::os::unix::fs::symlink(&source, &temporary)
                .map_err(|error| format!("{}: {error}", temporary.display()))?;
            fs::rename(&temporary, global)
                .map_err(|error| format!("{}: {error}", global.display()))?;
        } else {
            let bytes =
                fs::read(&source).map_err(|error| format!("{}: {error}", source.display()))?;
            write_private(global, &bytes, 0o755)?;
        }
        match host.binary_commit(global).as_deref() {
            Some(commit) if commit == candidate => Ok(()),
            other => Err(format!(
                "the moved global binary reports {} instead of {candidate}",
                other.unwrap_or("no commit")
            )),
        }
    })();
    if let Err(error) = moved {
        let restored = match &link_target {
            Some(target) => {
                let temporary =
                    global.with_extension(format!("turnover.{}.tmp", std::process::id()));
                let _ = fs::remove_file(&temporary);
                std::os::unix::fs::symlink(target, &temporary)
                    .and_then(|()| fs::rename(&temporary, global))
                    .map_err(|restore| format!("{}: {restore}", global.display()))
            }
            None => fs::read(&backup)
                .map_err(|read| format!("{}: {read}", backup.display()))
                .and_then(|bytes| write_private(global, &bytes, 0o755)),
        };
        return Err(match restored {
            Ok(()) => format!("global binary move failed and was rolled back: {error}"),
            Err(restore) => {
                format!("global binary move failed ({error}) and its rollback failed: {restore}")
            }
        });
    }
    Ok(format!(
        "moved {} -> {} (backup {})",
        sha8(old_commit.as_deref()),
        sha8(Some(candidate)),
        backup.display()
    ))
}

fn write_private(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
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

/// The turnover report: (JSON document, text, exit code). Exit codes:
/// report mode 0 all current, 74 some behind, 75 errors or unknowns;
/// turnover 0 done, 1 a refresh failed and rolled back, 4 a rollback
/// failed (the refresh codes), 75 when nothing could move (build or pin
/// or discovery failure).
#[derive(Debug)]
pub struct TurnoverReport {
    /// The JSON report (schema agentctl-services-turnover/v1).
    pub document: Value,
    /// One line per unit, then the summary.
    pub text: String,
    /// Process exit code, as documented above.
    pub exit_code: i32,
}

fn decide(
    options: &TurnoverOptions,
    candidate: &str,
    kind_paths: &[String],
    running: Option<&str>,
) -> (Decision, Vec<String>, String) {
    let Some(running) = running else {
        return (
            Decision::Unknown,
            Vec::new(),
            "the binary reports no commit; move it under services with a snapshot build and refresh".to_owned(),
        );
    };
    if running == candidate {
        return (Decision::Unchanged, Vec::new(), String::new());
    }
    match change_test(&options.checkout, running, candidate, kind_paths) {
        Some((_, true)) => (
            Decision::Ahead,
            Vec::new(),
            "the running commit is ahead of the candidate".to_owned(),
        ),
        Some((changed, _)) if changed.is_empty() => (Decision::Unchanged, changed, String::new()),
        Some((changed, _)) => (Decision::Refresh, changed, String::new()),
        None => (
            Decision::Unknown,
            Vec::new(),
            format!(
                "commits {running} and {candidate} cannot be compared in {}",
                options.checkout.display()
            ),
        ),
    }
}

/// Run `services turnover` (or its report / dry run).
pub fn turnover(
    host: &dyn Host,
    tables: &dyn OptionTables,
    options: &TurnoverOptions,
) -> Result<TurnoverReport, String> {
    let candidate = snapshot::resolve_commit(&options.checkout, &options.revision)?;
    let (services, unreadable) = discover::discover_all(host, tables)?;
    // Pins are read for every registry in play before anything is decided,
    // so a malformed pin refuses the whole run untouched.
    let mut registries: Vec<PathBuf> = services
        .iter()
        .map(|service| discover::registry_identity(&service.registry))
        .collect();
    registries.sort();
    registries.dedup();
    let mut pins: BTreeMap<PathBuf, Pins> = BTreeMap::new();
    for registry in &registries {
        let units: Vec<&str> = services
            .iter()
            .filter(|service| discover::registry_identity(&service.registry) == *registry)
            .map(|service| service.unit.as_str())
            .collect();
        pins.insert(registry.clone(), read_pins(host, registry, &units)?);
    }
    let global_pin = pin_reason(host, &options.root.join("global.pin"))?;
    if !options.report_only && !options.dry_run {
        for registry in &registries {
            tidy_pin_dir(registry);
        }
    }

    let mut rows: Vec<Row> = Vec::new();
    for service in &services {
        let registry = discover::registry_identity(&service.registry);
        let running = host.binary_commit(&service.binary);
        let paths: Vec<String> = affecting_paths(service.kind)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (mut decision, changed, mut detail) =
            decide(options, &candidate, &paths, running.as_deref());
        if let Some(reason) = pins
            .get(&registry)
            .and_then(|pins| pins.reason_for(&service.unit))
        {
            decision = Decision::Pinned;
            detail = format!("pinned: {reason}");
        } else if decision == Decision::Unchanged && running.as_deref() != Some(candidate.as_str())
        {
            detail = format!("unchanged since {}", sha8(running.as_deref()));
        }
        rows.push(Row {
            kind_label: service.kind.label().to_owned(),
            registry: Some(registry),
            unit: service.unit.clone(),
            binary: service.binary.clone(),
            running,
            changed_paths: changed,
            decision,
            detail,
            service: Some(service.clone()),
        });
    }
    let agentctl_units: Vec<&str> = services
        .iter()
        .map(|service| service.unit.as_str())
        .collect();
    for (unit, binary) in other_units(host) {
        if agentctl_units.contains(&unit.as_str()) {
            continue;
        }
        let name = binary
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let running = host.binary_commit(&binary);
        let paths = other_affecting_paths(&name);
        let (decision, changed, mut detail) =
            decide(options, &candidate, &paths, running.as_deref());
        // #1220: no snapshot can carry this binary, so it is report-only
        // whatever its binary reports — including no commit at all, which
        // must not force every report to exit 75.
        let decision = {
            if decision == Decision::Refresh || decision == Decision::Unknown {
                detail = "no snapshot carries this binary; rebuild it from the checkout".to_owned();
            }
            Decision::ReportOnly
        };
        rows.push(Row {
            kind_label: name,
            registry: None,
            unit,
            binary,
            running,
            changed_paths: changed,
            decision,
            detail,
            service: None,
        });
    }
    // The global binary rides along as a pseudo-unit, decided like the rest.
    let mut global_row: Option<Row> = None;
    if let Some(global) = options.global_binary.as_deref() {
        let running = host.binary_commit(global);
        let paths: Vec<String> = affecting_paths(Kind::Bridge)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (mut decision, changed, mut detail) =
            decide(options, &candidate, &paths, running.as_deref());
        if let Some(reason) = &global_pin {
            decision = Decision::Pinned;
            detail = format!("pinned: {reason}");
        } else if decision == Decision::Unchanged && running.as_deref() != Some(candidate.as_str())
        {
            detail = format!("unchanged since {}", sha8(running.as_deref()));
        }
        global_row = Some(Row {
            kind_label: "global".to_owned(),
            registry: None,
            unit: global.display().to_string(),
            binary: global.to_path_buf(),
            running,
            changed_paths: changed,
            decision,
            detail,
            service: None,
        });
    }

    let mut notes: Vec<String> = unreadable
        .iter()
        .map(|(unit, error)| format!("{unit}: runs agentctl but cannot be read: {error}"))
        .collect();
    let mut refresh_documents: Vec<Value> = Vec::new();
    let mut refresh_text = String::new();
    let mut exit_code = 0_i32;
    let any_refresh = rows.iter().any(|row| row.decision == Decision::Refresh)
        || global_row
            .as_ref()
            .is_some_and(|row| row.decision == Decision::Refresh);

    if options.dry_run && !options.report_only {
        // The ordered plan: bridges last per registry, global last of all.
        let mut plan_lines = Vec::new();
        for registry in &registries {
            let command = notify_command(host, registry, options.notify.as_deref());
            let mine: Vec<&Row> = rows
                .iter()
                .filter(|row| row.registry.as_deref() == Some(registry.as_path()))
                .collect();
            if mine.iter().any(|row| row.decision == Decision::Refresh) {
                if let Some(command) = &command {
                    plan_lines.push(format!("notify {} (registry start)", command.join(" ")));
                }
            }
            for row in ordered(&mine) {
                plan_lines.push(format!(
                    "{} {} {} -> {} {}",
                    row.decision.label(),
                    row.unit,
                    sha8(row.running.as_deref()),
                    sha8(Some(&candidate)),
                    row.detail
                ));
            }
        }
        for row in rows.iter().filter(|row| row.registry.is_none()) {
            plan_lines.push(format!("report-only {} ({})", row.unit, row.detail));
        }
        if let Some(row) = &global_row {
            plan_lines.push(format!(
                "{} global {} {} -> {}",
                row.decision.label(),
                row.unit,
                sha8(row.running.as_deref()),
                sha8(Some(&candidate))
            ));
        }
        notes.extend(plan_lines.into_iter().map(|line| format!("plan: {line}")));
    }

    if !options.report_only && !options.dry_run && any_refresh {
        let (snapshot, _built) = snapshot::build(
            host,
            &BuildOptions {
                root: options.root.clone(),
                checkout: options.checkout.clone(),
                revision: options.revision.clone(),
                cargo: options.cargo.clone(),
                work_dir: options.work_dir.clone(),
                keep_work: options.keep_work,
            },
        )?;
        if snapshot.commit != candidate {
            return Err(format!(
                "the built snapshot is {} but turnover decided against {candidate}",
                snapshot.commit
            ));
        }
        let mut stopped = false;
        for registry in &registries {
            if stopped {
                break;
            }
            let mine: Vec<&Row> = rows
                .iter()
                .filter(|row| {
                    row.registry.as_deref() == Some(registry.as_path())
                        && row.decision == Decision::Refresh
                })
                .collect();
            if mine.is_empty() {
                continue;
            }
            let command = notify_command(host, registry, options.notify.as_deref());
            let first = mine[0];
            notify(
                host,
                command.as_deref(),
                "registry-start",
                registry,
                &first.unit,
                first.running.as_deref(),
                &candidate,
                "starting",
                &mut notes,
            );
            // Bridges last within the registry: two refresh calls.
            let (bridges, rest): (Vec<&Row>, Vec<&Row>) = mine
                .iter()
                .partition(|row| row.service.as_ref().map(|s| s.kind) == Some(Kind::Bridge));
            let mut verdicts: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
            for group in [&rest, &bridges] {
                if group.is_empty() || stopped {
                    continue;
                }
                let only: Vec<String> = group.iter().map(|row| row.unit.clone()).collect();
                let refresh_options = RefreshOptions {
                    snapshot: snapshot.clone(),
                    only,
                    dry_run: false,
                    force: false,
                    health_timeout: options.health_timeout,
                    headless_wait: options.headless_wait,
                    refreshes_dir: options.root.join("refreshes"),
                    drop_in_dir: options.drop_in_dir.clone(),
                    interrupted: options.interrupted.clone(),
                };
                let report =
                    super::refresh(host, tables, registry, &options.root, &refresh_options)?;
                refresh_text.push_str(&report.text);
                exit_code = exit_code.max(report.exit_code);
                if report.exit_code != 0 {
                    stopped = true;
                }
                for entry in report.document["services"].as_array().into_iter().flatten() {
                    if let Some(unit) = entry["unit"].as_str() {
                        verdicts.insert(
                            unit.to_owned(),
                            (
                                entry["action"].as_str().unwrap_or("?").to_owned(),
                                entry["old"]["commit"].as_str().map(str::to_owned),
                            ),
                        );
                    }
                }
                refresh_documents.push(report.document);
            }
            for row in ordered(&mine) {
                let (verdict, old) = verdicts
                    .get(&row.unit)
                    .cloned()
                    .unwrap_or_else(|| ("not-attempted".to_owned(), row.running.clone()));
                notify(
                    host,
                    command.as_deref(),
                    "unit-done",
                    registry,
                    &row.unit,
                    old.as_deref(),
                    &candidate,
                    &verdict,
                    &mut notes,
                );
            }
        }
        // The global binary moves last of all, and only if nothing failed.
        if !stopped && exit_code == 0 {
            if let Some(row) = &global_row {
                if row.decision == Decision::Refresh {
                    match move_global_binary(host, options, &snapshot, &candidate) {
                        Ok(detail) => notes.push(format!("global binary: {detail}")),
                        Err(error) => {
                            notes.push(format!("global binary: {error}"));
                            exit_code = exit_code.max(1);
                        }
                    }
                }
            }
        }
    }

    if options.report_only {
        let errors = !unreadable.is_empty()
            || rows.iter().any(|row| row.decision == Decision::Unknown)
            || global_row
                .as_ref()
                .is_some_and(|row| row.decision == Decision::Unknown);
        let behind = rows.iter().any(|row| {
            matches!(row.decision, Decision::Refresh | Decision::ReportOnly)
                && !row.changed_paths.is_empty()
        }) || global_row
            .as_ref()
            .is_some_and(|row| row.decision == Decision::Refresh);
        exit_code = if errors {
            75
        } else if behind {
            74
        } else {
            0
        };
    }

    // #1220: a failing scheduled outcome alerts the coordinator through
    // the same notify mechanism, once per registry.
    if !options.dry_run && scheduled_outcome(exit_code) == ScheduledOutcome::Failure {
        for registry in &registries {
            let command = notify_command(host, registry, options.notify.as_deref());
            notify(
                host,
                command.as_deref(),
                "run-failed",
                registry,
                "",
                None,
                &candidate,
                &format!("exit-{exit_code}"),
                &mut notes,
            );
        }
    }

    Ok(render(
        candidate,
        &rows,
        global_row.as_ref(),
        &unreadable,
        &notes,
        &refresh_documents,
        &refresh_text,
        options,
        exit_code,
    ))
}

/// Refresh order inside one registry: restore, watchers and headless
/// first, bridges last (the owner's channel stays up longest).
fn ordered<'a>(rows: &[&'a Row]) -> Vec<&'a Row> {
    let mut ordered: Vec<&Row> = rows.to_vec();
    ordered.sort_by_key(
        |row| match row.service.as_ref().map(|service| service.kind) {
            Some(Kind::Bridge) => 1,
            _ => 0,
        },
    );
    ordered
}

#[allow(clippy::too_many_arguments)]
fn render(
    candidate: String,
    rows: &[Row],
    global_row: Option<&Row>,
    unreadable: &[(String, String)],
    notes: &[String],
    refresh_documents: &[Value],
    refresh_text: &str,
    options: &TurnoverOptions,
    exit_code: i32,
) -> TurnoverReport {
    let row_json = |row: &Row| {
        json!({
            "kind": row.kind_label,
            "registry": row.registry,
            "unit": row.unit,
            "binary": row.binary,
            "running_commit": row.running,
            "candidate_commit": candidate,
            "changed_paths": row.changed_paths,
            "changed_paths_count": row.changed_paths.len(),
            "decision": row.decision.label(),
            "detail": row.detail,
        })
    };
    let mut services: Vec<Value> = rows.iter().map(row_json).collect();
    if let Some(row) = global_row {
        services.push(row_json(row));
    }
    let behind = services
        .iter()
        .filter(|row| row["decision"] == "refresh")
        .count();
    let document = json!({
        "schema": "agentctl-services-turnover/v1",
        "candidate_commit": candidate,
        "report_only": options.report_only,
        "dry_run": options.dry_run,
        "services": services,
        "unreadable": unreadable.iter().map(|(unit, error)| json!({"unit": unit, "error": error})).collect::<Vec<_>>(),
        "notes": notes,
        "refreshes": refresh_documents,
        "behind": behind,
        "exit_code": exit_code,
    });
    let mut text = String::new();
    for row in rows.iter().chain(global_row) {
        let _ = writeln!(
            text,
            "{} {} {} {}..{} {} changed {}",
            row.kind_label,
            row.registry
                .as_ref()
                .map_or_else(|| "-".to_owned(), |path| path.display().to_string()),
            row.unit,
            sha8(row.running.as_deref()),
            sha8(Some(&candidate)),
            row.changed_paths.len(),
            row.decision.label()
        );
        if !row.detail.is_empty() {
            let _ = writeln!(text, "  {}", row.detail);
        }
    }
    for (unit, error) in unreadable {
        let _ = writeln!(text, "{unit}: runs agentctl but cannot be read: {error}");
    }
    for note in notes {
        let _ = writeln!(text, "{note}");
    }
    text.push_str(refresh_text);
    let _ = writeln!(
        text,
        "turnover: {} service(s), {} to refresh, candidate {}",
        services.len(),
        behind,
        sha8(Some(&candidate))
    );
    TurnoverReport {
        document,
        text,
        exit_code,
    }
}
