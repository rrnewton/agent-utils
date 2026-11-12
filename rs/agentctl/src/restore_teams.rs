//! Restore every configured team on the host in one deterministic command
//! (#1128 restore-deterministic-script).
//!
//! `agentctl restore-teams` owns only host sequencing and reporting. The
//! per-registry revive stays in [`crate::restore::restore`]: this module
//! waits (bounded) for each registry's configured user certificate, calls
//! `restore` once per registry in config order, and aggregates the
//! per-registry reports into one JSON report plus one human per-agent
//! table. It never starts a competing terminal server: starting the
//! configured Herdr unit, when Herdr is down, is the per-registry
//! restore's own step, exactly as a single-registry run does it.
//!
//! There is no host-wide discovery. The registries are named explicitly,
//! by a teams file (`--config`) and/or repeated `--registry` filters on
//! the subcommand; with neither, the command is a usage error. When both
//! are given, `--registry` only limits the config's list, so a live run
//! can be confined to one registry (its records, panes, and reports)
//! without touching any other team.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::AgentError;
use crate::restore::{read_report, Host, RestoreConfig, RestoreOptions};
use crate::subagents::{ManagedAgents, ManagedApi};

/// Schema of the teams file `--config` reads.
pub const TEAMS_SCHEMA: &str = "agentctl-restore-teams/v1";
/// Schema of the aggregated report this command prints.
pub const TEAMS_REPORT_SCHEMA: &str = "agentctl-restore-teams-report/v1";

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// One registry to restore, in config order.
#[derive(Clone, Debug, PartialEq)]
pub struct RegistryEntry {
    /// The agent registry directory (absolute).
    pub registry: PathBuf,
    /// The Herdr executable for this registry (absolute), when the teams
    /// file pins one; otherwise the command's `--herdr-bin` applies.
    pub herdr_bin: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamsFile {
    schema: String,
    registries: Vec<TeamsEntryRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TeamsEntryRaw {
    Path(PathBuf),
    Object(TeamsEntryObject),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamsEntryObject {
    registry: PathBuf,
    #[serde(default)]
    herdr_bin: Option<PathBuf>,
}

/// The parsed teams file: an ordered, validated registry list.
#[derive(Clone, Debug, PartialEq)]
pub struct TeamsConfig {
    /// The registries, in file order.
    pub entries: Vec<RegistryEntry>,
}

impl TeamsConfig {
    /// Read and validate a teams file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .map_err(|error| fail(format!("cannot read {}: {error}", path.display())))?;
        Self::parse(&text, path)
    }

    /// Parse a teams file's text; `path` only names it in errors.
    pub fn parse(text: &str, path: &Path) -> Result<Self> {
        let invalid = |detail: String| fail(format!("invalid {} ({detail})", path.display()));
        let file: TeamsFile =
            serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
        if file.schema != TEAMS_SCHEMA {
            return Err(invalid(format!("schema must be {TEAMS_SCHEMA}")));
        }
        if file.registries.is_empty() {
            return Err(invalid("registries must not be empty".to_owned()));
        }
        let mut entries: Vec<RegistryEntry> = Vec::new();
        for raw in file.registries {
            let entry = match raw {
                TeamsEntryRaw::Path(registry) => RegistryEntry {
                    registry,
                    herdr_bin: None,
                },
                TeamsEntryRaw::Object(object) => RegistryEntry {
                    registry: object.registry,
                    herdr_bin: object.herdr_bin,
                },
            };
            if !entry.registry.is_absolute() {
                return Err(invalid(format!(
                    "registry must be absolute: {}",
                    entry.registry.display()
                )));
            }
            if let Some(herdr_bin) = entry.herdr_bin.as_ref() {
                if !herdr_bin.is_absolute() {
                    return Err(invalid(format!(
                        "herdr_bin must be absolute: {}",
                        herdr_bin.display()
                    )));
                }
            }
            if entries
                .iter()
                .any(|known: &RegistryEntry| known.registry == entry.registry)
            {
                return Err(invalid(format!(
                    "duplicate registry: {}",
                    entry.registry.display()
                )));
            }
            entries.push(entry);
        }
        Ok(Self { entries })
    }
}

/// Resolve the registries to restore from `--config` and/or the repeated
/// `--registry` filters. Neither is a usage error naming the remedy:
/// there is deliberately no host-wide discovery default. With both, the
/// filters only limit the config's list (a filter the config does not
/// name is an error, so a limited run can never silently widen). With
/// only filters, they are the list, in the order given. `cli_registries`
/// must already be absolute (the CLI resolves them against its cwd).
pub fn resolve_entries(
    config: Option<&TeamsConfig>,
    cli_registries: &[PathBuf],
) -> Result<Vec<RegistryEntry>> {
    if config.is_none() && cli_registries.is_empty() {
        return Err(fail(
            "restore-teams needs at least one registry: pass --config TEAMS.json and/or \
             repeat --registry DIR after restore-teams (there is no host-wide discovery)",
        ));
    }
    let Some(config) = config else {
        let mut entries: Vec<RegistryEntry> = Vec::new();
        for registry in cli_registries {
            if !entries.iter().any(|known| &known.registry == registry) {
                entries.push(RegistryEntry {
                    registry: registry.clone(),
                    herdr_bin: None,
                });
            }
        }
        return Ok(entries);
    };
    if cli_registries.is_empty() {
        return Ok(config.entries.clone());
    }
    for registry in cli_registries {
        if !config
            .entries
            .iter()
            .any(|known| &known.registry == registry)
        {
            return Err(fail(format!(
                "--registry {} is not named by the teams config; with --config, \
                 --registry only limits the config's registries (drop --config to \
                 restore exactly the --registry list)",
                registry.display()
            )));
        }
    }
    let entries: Vec<RegistryEntry> = config
        .entries
        .iter()
        .filter(|known| cli_registries.contains(&known.registry))
        .cloned()
        .collect();
    if entries.is_empty() {
        return Err(fail(
            "restore-teams: the --registry filters matched no registry in the teams config",
        ));
    }
    Ok(entries)
}

/// How `restore-teams` was asked to run.
#[derive(Clone, Debug)]
pub struct RestoreTeamsOptions {
    /// Mark each per-registry report as the boot hook's run.
    pub boot: bool,
    /// Run each registry even when this boot's report says it completed.
    pub force: bool,
    /// Report the plan per registry without reviving anything.
    pub dry_run: bool,
    /// Startup-readiness deadline for each revived harness.
    pub startup_timeout: Duration,
    /// Bounded wait for a registry's configured user certificate before
    /// that registry is left unlaunched and counted 75.
    pub cert_wait: Duration,
    /// The default Herdr executable (the global `--herdr-bin`), used for
    /// registries whose entry pins none.
    pub herdr_bin: PathBuf,
}

/// The aggregated outcome: the JSON report, the process exit code, and
/// the human table lines (printed to standard error by the CLI).
#[derive(Clone, Debug)]
pub struct RestoreTeamsOutcome {
    /// Schema `agentctl-restore-teams-report/v1`.
    pub report: Value,
    /// 75 if any registry is incomplete or certificate-gated, else 74 if
    /// any lane was refused, else 0; a dry run is 0.
    pub exit_code: i32,
    /// One line per agent across every registry, plus a status line for
    /// any registry that produced no rows; replayed rows end `(replayed)`.
    pub table: Vec<String>,
}

/// The remedy a missing user certificate names: the registry's own fetch
/// command when it configures one, else naming the file to fetch to.
/// Mirrors the wording `restore` itself records.
fn certificate_remedy(config: &RestoreConfig, path: &Path) -> String {
    config
        .certificate_fetch
        .clone()
        .unwrap_or_else(|| format!("fetch the user certificate to {}", path.display()))
}

/// Poll for `path` to become a file, sleeping through `host`, until
/// `wait` has elapsed. Presence is what `restore` itself checks, so it
/// is what the wait checks; expiry policy stays with the fetch step a
/// host shim runs before this command. A zero wait checks once.
fn wait_for_certificate(host: &dyn Host, path: &Path, wait: Duration) -> bool {
    let started = host.now();
    loop {
        if path.is_file() {
            return true;
        }
        let elapsed = host.now() - started;
        if elapsed >= wait.as_secs_f64() {
            return false;
        }
        let remaining = wait.as_secs_f64() - elapsed;
        host.sleep(Duration::from_secs_f64(remaining.min(2.0)));
    }
}

/// Where a registry's recorded report for `boot_id` may live, in read
/// order (#1359): the configured `report_dir` or the per-registry
/// `restore/reports/` subdirectory first, then the earlier locations
/// (`restore-reports/`, `agents/restore-reports/`), then the shared
/// state-directory default. The shared default is only ever a fallback for
/// one registry's own recorded outcome, never the basis of a multi-registry
/// decision on its own: it is tried last, after the registry's own locations.
pub(crate) fn candidate_report_paths(
    registry: &Path,
    config: &RestoreConfig,
    boot_id: &str,
) -> Vec<PathBuf> {
    crate::restore::candidate_report_paths(registry, config, boot_id)
}

/// The first recorded report for this boot that reads, with its path.
pub(crate) fn read_recorded_report(
    registry: &Path,
    config: &RestoreConfig,
    boot_id: &str,
) -> Option<(PathBuf, Value)> {
    candidate_report_paths(registry, config, boot_id)
        .into_iter()
        .find_map(|path| read_report(&path).map(|report| (path, report)))
}

/// One table row per agent in a per-registry report: the coordinator
/// first, then the lanes, each marked replayed when the run only
/// re-read this boot's recorded outcome.
fn report_rows(registry: &Path, report: &Value, replayed: bool) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut push = |row: &Value, fallback_name: &str| {
        if !row.is_object() {
            return;
        }
        let mut out = json!({
            "registry": registry,
            "name": row["name"].as_str().unwrap_or(fallback_name),
            "action": row["action"].as_str().unwrap_or("?"),
            "replayed": replayed,
        });
        for key in ["reason", "disposition", "pane_id", "slot_owner"] {
            if !row[key].is_null() {
                out[key] = row[key].clone();
            }
        }
        rows.push(out);
    };
    push(&report["coordinator"], "coordinator");
    for row in report["agents"].as_array().into_iter().flatten() {
        push(row, "?");
    }
    rows
}

/// The human table line for one row. Reasons are flattened to one line;
/// the JSON report keeps them whole.
fn table_line(row: &Value) -> String {
    let mut line = format!(
        "agentctl restore-teams: {} {} {}",
        row["registry"].as_str().unwrap_or("?"),
        row["name"].as_str().unwrap_or("?"),
        row["action"].as_str().unwrap_or("?")
    );
    if let Some(reason) = row["reason"].as_str().filter(|reason| !reason.is_empty()) {
        line.push(' ');
        line.push_str(&reason.replace(['\n', '\r'], " "));
    }
    if row["replayed"] == true {
        line.push_str(" (replayed)");
    }
    line
}

/// How many lanes a report refused: its recorded count when present,
/// else its refused rows.
fn refused_count(report: &Value, rows: &[Value]) -> u64 {
    report["refused"]
        .as_u64()
        .unwrap_or_else(|| rows.iter().filter(|row| row["action"] == "refused").count() as u64)
}

/// A report's class: `complete` (0), `refused` (74: finished, with named
/// refused lanes a boot unit must not retry), or `incomplete` (75). A
/// dry run classifies by what the same run for real would do; the
/// aggregate exit of a dry run is still 0.
fn classify_report(report: &Value, rows: &[Value], dry_run: bool) -> (&'static str, i32) {
    let refused = refused_count(report, rows);
    if dry_run {
        if refused > 0 {
            return ("refused", 74);
        }
        if report["would_complete"] == true || report["complete"] == true {
            return ("complete", 0);
        }
        return ("incomplete", 75);
    }
    if report["complete"] == true {
        if refused > 0 {
            ("refused", 74)
        } else {
            ("complete", 0)
        }
    } else {
        ("incomplete", 75)
    }
}

/// Restore one registry with an already-built client: follow the
/// registry's coordinator designation exactly as `restore` does (a
/// disagreement never stops the restore; it is recorded in the
/// report), run it, and stamp the trigger.
pub(crate) fn restore_one_with_client<A: ManagedApi + ?Sized>(
    client: &A,
    host: &dyn Host,
    registry: &Path,
    config: &mut RestoreConfig,
    options: &RestoreOptions,
    boot: bool,
) -> Result<Value> {
    let designation_problem = match ManagedAgents::new(client, registry)
        .and_then(|manager| manager.designated_coordinator())
    {
        Ok(designated) => config
            .follow_designation(designated)
            .err()
            .map(|error| error.to_string()),
        Err(error) => Some(format!(
            "the coordinator designation is unreadable: {error}"
        )),
    };
    let mut report = crate::restore::restore(client, registry, host, config, options)?;
    report["trigger"] = json!(if boot { "boot" } else { "manual" });
    if let Some(problem) = designation_problem {
        report["designation_problem"] = json!(problem);
    }
    Ok(report)
}

/// Restore every entry in order and aggregate the outcomes. `runner`
/// performs one registry's restore (the CLI builds that registry's
/// Herdr client and calls [`restore_one_with_client`]; tests pass a
/// fake client the same way). A registry whose config cannot load,
/// whose certificate never arrives within the bounded wait, or whose
/// restore errors is recorded and counted 75, and the remaining
/// registries still run; a certificate-gated registry launches nothing,
/// while registries with no certificate requirement proceed. Terminal
/// server startup is never done here: each per-registry restore waits
/// for Herdr and starts its configured Herdr unit itself, as it does
/// standalone.
pub fn restore_teams<R>(
    host: &dyn Host,
    entries: &[RegistryEntry],
    options: &RestoreTeamsOptions,
    mut runner: R,
) -> Result<RestoreTeamsOutcome>
where
    R: FnMut(&RegistryEntry, &mut RestoreConfig, &RestoreOptions) -> Result<Value>,
{
    if entries.is_empty() {
        return Err(fail(
            "restore-teams needs at least one registry: pass --config TEAMS.json and/or \
             repeat --registry DIR after restore-teams",
        ));
    }
    let boot_id = host.boot_id()?;
    let mut registries: Vec<Value> = Vec::new();
    let mut all_rows: Vec<Value> = Vec::new();
    let mut table: Vec<String> = Vec::new();
    let mut refused_lanes: Vec<Value> = Vec::new();
    let mut refused_total: u64 = 0;
    let mut any_incomplete = false;
    let mut any_refused = false;
    // Registries on one host name the same user certificate file; a
    // file already waited out is not waited out again per registry.
    let mut certificate_timeouts: Vec<PathBuf> = Vec::new();
    let mut certificates_present: Vec<PathBuf> = Vec::new();

    for entry in entries {
        let registry = entry.registry.as_path();
        let herdr_bin = entry
            .herdr_bin
            .clone()
            .unwrap_or_else(|| options.herdr_bin.clone());
        let mut config = match RestoreConfig::load(registry) {
            Ok(config) => config,
            Err(error) => {
                any_incomplete = true;
                table.push(format!(
                    "agentctl restore-teams: {} error {}",
                    registry.display(),
                    error.to_string().replace(['\n', '\r'], " ")
                ));
                registries.push(json!({
                    "registry": registry,
                    "herdr_bin": herdr_bin,
                    "status": "error",
                    "exit_code": 75,
                    "replayed": false,
                    "error": error.to_string(),
                    "report": Value::Null,
                    "rows": [],
                }));
                continue;
            }
        };
        if let Some(certificate) = config.certificate_file.clone() {
            if !options.dry_run {
                let present = if certificates_present.contains(&certificate) {
                    true
                } else if certificate_timeouts.contains(&certificate) {
                    false
                } else if wait_for_certificate(host, &certificate, options.cert_wait) {
                    certificates_present.push(certificate.clone());
                    true
                } else {
                    certificate_timeouts.push(certificate.clone());
                    false
                };
                if !present {
                    let remedy = certificate_remedy(&config, &certificate);
                    any_incomplete = true;
                    table.push(format!(
                        "agentctl restore-teams: {} certificate-timeout user certificate \
                         missing at {}; remedy: {}",
                        registry.display(),
                        certificate.display(),
                        remedy.replace(['\n', '\r'], " ")
                    ));
                    registries.push(json!({
                        "registry": registry,
                        "herdr_bin": herdr_bin,
                        "status": "certificate-timeout",
                        "exit_code": 75,
                        "replayed": false,
                        "certificate": {
                            "present": false,
                            "path": certificate,
                            "remedy": remedy,
                            "waited_seconds": options.cert_wait.as_secs(),
                        },
                        "report": Value::Null,
                        "rows": [],
                    }));
                    continue;
                }
            }
        }
        let restore_options = RestoreOptions {
            force: options.force,
            dry_run: options.dry_run,
            startup_timeout: options.startup_timeout,
            wrkslots: crate::subagents::project_wrkslots(registry),
            herdr_bin: herdr_bin.clone(),
        };
        let runner_report = match runner(entry, &mut config, &restore_options) {
            Ok(report) => report,
            Err(error) => {
                any_incomplete = true;
                table.push(format!(
                    "agentctl restore-teams: {} error {}",
                    registry.display(),
                    error.to_string().replace(['\n', '\r'], " ")
                ));
                registries.push(json!({
                    "registry": registry,
                    "herdr_bin": herdr_bin,
                    "status": "error",
                    "exit_code": 75,
                    "replayed": false,
                    "error": error.to_string(),
                    "report": Value::Null,
                    "rows": [],
                }));
                continue;
            }
        };
        let outcome = finish_registry(
            registry,
            &config,
            &boot_id,
            herdr_bin,
            runner_report,
            options.dry_run,
        );
        match outcome.exit_code {
            75 => any_incomplete = true,
            74 => any_refused = true,
            _ => {}
        }
        refused_total += outcome.refused;
        refused_lanes.extend(outcome.refused_lanes);
        table.extend(outcome.lines);
        all_rows.extend(outcome.rows);
        registries.push(outcome.entry);
    }

    let exit_code = if options.dry_run {
        0
    } else if any_incomplete {
        75
    } else if any_refused {
        74
    } else {
        0
    };
    let finished = registries
        .iter()
        .all(|entry| matches!(entry["status"].as_str(), Some("complete" | "refused")));
    let report = json!({
        "schema": TEAMS_REPORT_SCHEMA,
        "boot_id": boot_id,
        "trigger": if options.boot { "boot" } else { "manual" },
        "dry_run": options.dry_run,
        "complete": !options.dry_run && finished,
        "would_complete": registries.iter().all(|entry| entry["status"] == "complete"),
        "refused": refused_total,
        "refused_lanes": refused_lanes,
        "exit_code": exit_code,
        "registries": registries,
        "rows": all_rows,
    });
    Ok(RestoreTeamsOutcome {
        report,
        exit_code,
        table,
    })
}

/// One registry's finished piece of the aggregate.
struct RegistryOutcome {
    entry: Value,
    rows: Vec<Value>,
    lines: Vec<String>,
    refused_lanes: Vec<Value>,
    refused: u64,
    exit_code: i32,
}

/// Turn one registry's runner report into its aggregate entry. A report
/// that only says `already-restored` carries no per-agent outcomes, so
/// the recorded report for the boot is read through the dual-location
/// helper and its rows are replayed, labelled as such; a runner that
/// already returns the recorded rows is used as-is. An
/// `already-restored` whose recorded report cannot be read is counted
/// incomplete, never as a fresh empty success.
fn finish_registry(
    registry: &Path,
    config: &RestoreConfig,
    boot_id: &str,
    herdr_bin: PathBuf,
    runner_report: Value,
    dry_run: bool,
) -> RegistryOutcome {
    let mut replayed =
        runner_report["action"] == "already-restored" || runner_report["replayed"] == true;
    let mut effective = runner_report.clone();
    let mut report_path = runner_report["report"].as_str().map(PathBuf::from);
    let has_rows = effective["agents"].is_array() || effective["coordinator"].is_object();
    if replayed && !has_rows {
        let recorded_boot = runner_report["boot_id"].as_str().unwrap_or(boot_id);
        let recorded = read_recorded_report(registry, config, recorded_boot).or_else(|| {
            let path = report_path.clone()?;
            read_report(&path).map(|report| (path, report))
        });
        match recorded {
            Some((path, report))
                if report["agents"].is_array()
                    || report["coordinator"].is_object()
                    || report["complete"] == true =>
            {
                effective = report;
                report_path = Some(path);
            }
            _ => {
                let error = "this boot's restore already completed but its recorded \
                             report could not be read, so its per-agent outcomes are \
                             unknown"
                    .to_owned();
                return RegistryOutcome {
                    entry: json!({
                        "registry": registry,
                        "herdr_bin": herdr_bin,
                        "status": "incomplete",
                        "exit_code": 75,
                        "replayed": true,
                        "error": error,
                        "report": runner_report,
                        "report_path": report_path,
                        "rows": [],
                    }),
                    rows: Vec::new(),
                    lines: vec![format!(
                        "agentctl restore-teams: {} incomplete {error}",
                        registry.display()
                    )],
                    refused_lanes: Vec::new(),
                    refused: 0,
                    exit_code: 75,
                };
            }
        }
    }
    if effective["action"] == "already-restored" {
        replayed = true;
    }
    if report_path.is_none() {
        report_path = effective["report"].as_str().map(PathBuf::from);
    }
    let rows = report_rows(registry, &effective, replayed);
    let lines: Vec<String> = rows.iter().map(table_line).collect();
    let (status, exit_code) = classify_report(&effective, &rows, dry_run);
    let refused = refused_count(&effective, &rows);
    let refused_lanes: Vec<Value> = rows
        .iter()
        .filter(|row| row["action"] == "refused")
        .map(|row| json!({"registry": registry, "name": row["name"]}))
        .collect();
    let mut entry = json!({
        "registry": registry,
        "herdr_bin": herdr_bin,
        "status": status,
        "exit_code": exit_code,
        "replayed": replayed,
        "report": effective,
        "report_path": report_path,
        "rows": rows.clone(),
    });
    if entry["report"] != runner_report {
        entry["runner_report"] = runner_report;
    }
    RegistryOutcome {
        entry,
        rows,
        lines,
        refused_lanes,
        refused,
        exit_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::restore::CommandOutcome;
    use std::cell::Cell;

    const BOOT: &str = "11111111-2222-3333-4444-555555555555";

    /// A host whose clock only moves when it sleeps; it can create the
    /// certificate file after a counted number of sleeps.
    struct TestHost {
        now: Cell<f64>,
        sleeps: Cell<u32>,
        create_after: Option<(u32, PathBuf)>,
    }

    impl TestHost {
        fn new() -> Self {
            Self {
                now: Cell::new(1_000.0),
                sleeps: Cell::new(0),
                create_after: None,
            }
        }
    }

    impl Host for TestHost {
        fn boot_id(&self) -> Result<String> {
            Ok(BOOT.to_owned())
        }
        fn boot_time(&self) -> Option<u64> {
            None
        }
        fn now(&self) -> f64 {
            self.now.get()
        }
        fn sleep(&self, duration: Duration) {
            self.now.set(self.now.get() + duration.as_secs_f64());
            self.sleeps.set(self.sleeps.get() + 1);
            if let Some((after, path)) = self.create_after.as_ref() {
                if self.sleeps.get() >= *after {
                    let _ = fs::write(path, "cert");
                }
            }
        }
        fn run(&self, _argv: &[String], _cwd: Option<&Path>, _timeout: Duration) -> CommandOutcome {
            CommandOutcome {
                status: Some(0),
                ..CommandOutcome::default()
            }
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentctl-restore-teams-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn options() -> RestoreTeamsOptions {
        RestoreTeamsOptions {
            boot: false,
            force: false,
            dry_run: false,
            startup_timeout: Duration::from_secs(30),
            cert_wait: Duration::ZERO,
            herdr_bin: PathBuf::from("herdr"),
        }
    }

    #[test]
    fn teams_file_parses_paths_and_objects_and_refuses_the_rest() {
        let path = Path::new("/tmp/teams.json");
        let parsed = TeamsConfig::parse(
            r#"{"schema":"agentctl-restore-teams/v1","registries":[
                "/home/owner/one/.agentctl",
                {"registry":"/home/owner/two/.agentctl","herdr_bin":"/home/owner/bin/herdr"}
            ]}"#,
            path,
        )
        .unwrap();
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].herdr_bin, None);
        assert_eq!(
            parsed.entries[1].herdr_bin,
            Some(PathBuf::from("/home/owner/bin/herdr"))
        );
        for text in [
            r#"{"schema":"agentctl-restore-teams/v2","registries":["/a"]}"#,
            r#"{"schema":"agentctl-restore-teams/v1","registries":[]}"#,
            r#"{"schema":"agentctl-restore-teams/v1","registries":["relative"]}"#,
            r#"{"schema":"agentctl-restore-teams/v1","registries":[{"registry":"/a","herdr_bin":"herdr"}]}"#,
            r#"{"schema":"agentctl-restore-teams/v1","registries":["/a","/a"]}"#,
            r#"{"schema":"agentctl-restore-teams/v1","registries":["/a"],"extra":true}"#,
            r#"{"schema":"agentctl-restore-teams/v1","registries":[{"registry":"/a","extra":true}]}"#,
            r#"{"schema":"agentctl-restore-teams/v1"}"#,
        ] {
            assert!(TeamsConfig::parse(text, path).is_err(), "{text}");
        }
    }

    #[test]
    fn resolve_entries_needs_a_filter_and_only_limits_a_config() {
        let one = PathBuf::from("/home/owner/one/.agentctl");
        let two = PathBuf::from("/home/owner/two/.agentctl");
        let config = TeamsConfig {
            entries: vec![
                RegistryEntry {
                    registry: one.clone(),
                    herdr_bin: None,
                },
                RegistryEntry {
                    registry: two.clone(),
                    herdr_bin: None,
                },
            ],
        };
        // No filter: a usage error, never discovery.
        let error = resolve_entries(None, &[]).unwrap_err().to_string();
        assert!(error.contains("no host-wide discovery"), "{error}");
        // Filters alone are the list.
        let only = resolve_entries(None, std::slice::from_ref(&one)).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].registry, one);
        // A filter limits the config to just that registry.
        let limited = resolve_entries(Some(&config), std::slice::from_ref(&two)).unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].registry, two);
        // A filter outside the config never widens it.
        let other = PathBuf::from("/home/owner/other/.agentctl");
        assert!(resolve_entries(Some(&config), &[other]).is_err());
    }

    #[test]
    fn certificate_wait_is_bounded_and_sees_a_late_certificate() {
        let root = scratch("cert");
        let certificate = root.join("user.pem");
        let host = TestHost::new();
        assert!(!wait_for_certificate(&host, &certificate, Duration::ZERO));
        let host = TestHost {
            create_after: Some((2, certificate.clone())),
            ..TestHost::new()
        };
        assert!(wait_for_certificate(
            &host,
            &certificate,
            Duration::from_secs(30)
        ));
        assert!(certificate.is_file());
        let missing = root.join("never.pem");
        let host = TestHost::new();
        assert!(!wait_for_certificate(
            &host,
            &missing,
            Duration::from_secs(3)
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recorded_report_prefers_the_configured_dir_and_reads_legacy_without_one() {
        // #1359: one read order for restore and restore-teams — configured
        // or new default first, legacy locations after.
        let root = scratch("dual-read");
        let registry = root.join("registry");
        let reports = root.join("reports");
        fs::create_dir_all(registry.join("restore-reports")).unwrap();
        fs::create_dir_all(&reports).unwrap();
        fs::write(
            registry.join("restore.json"),
            json!({"schema": "agentctl-restore/v1", "report_dir": reports}).to_string(),
        )
        .unwrap();
        let config = RestoreConfig::load(&registry).unwrap();
        fs::write(
            reports.join(format!("{BOOT}.json")),
            json!({"schema": "agentctl-restore-report/v1", "complete": true,
                   "agents": [{"name": "shared", "action": "revived"}]})
            .to_string(),
        )
        .unwrap();
        let (path, report) = read_recorded_report(&registry, &config, BOOT).unwrap();
        assert_eq!(path, reports.join(format!("{BOOT}.json")));
        assert_eq!(report["agents"][0]["name"], "shared");
        // A legacy copy never shadows the configured directory.
        fs::write(
            registry
                .join("restore-reports")
                .join(format!("{BOOT}.json")),
            json!({"schema": "agentctl-restore-report/v1", "complete": true,
                   "agents": [{"name": "own", "action": "revived"}]})
            .to_string(),
        )
        .unwrap();
        let (path, report) = read_recorded_report(&registry, &config, BOOT).unwrap();
        assert_eq!(path, reports.join(format!("{BOOT}.json")));
        assert_eq!(report["agents"][0]["name"], "shared");
        // Without a configured directory, the legacy location still reads,
        // and the new default location wins once it has the report.
        fs::write(
            registry.join("restore.json"),
            json!({"schema": "agentctl-restore/v1"}).to_string(),
        )
        .unwrap();
        let config = RestoreConfig::load(&registry).unwrap();
        let (path, report) = read_recorded_report(&registry, &config, BOOT).unwrap();
        assert!(
            path.ends_with(format!("restore-reports/{BOOT}.json")),
            "{path:?}"
        );
        assert_eq!(report["agents"][0]["name"], "own");
        let current = crate::restore::reports_directory(&registry);
        fs::create_dir_all(&current).unwrap();
        fs::write(
            current.join(format!("{BOOT}.json")),
            json!({"schema": "agentctl-restore-report/v1", "complete": true,
                   "agents": [{"name": "current", "action": "revived"}]})
            .to_string(),
        )
        .unwrap();
        let (path, report) = read_recorded_report(&registry, &config, BOOT).unwrap();
        assert_eq!(path, current.join(format!("{BOOT}.json")));
        assert_eq!(report["agents"][0]["name"], "current");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn replayed_run_replays_the_recorded_rows_not_an_empty_success() {
        let root = scratch("replay");
        let registry = root.join("registry");
        let reports = root.join("reports");
        fs::create_dir_all(&registry).unwrap();
        fs::create_dir_all(&reports).unwrap();
        fs::write(
            registry.join("restore.json"),
            json!({"schema": "agentctl-restore/v1", "report_dir": reports}).to_string(),
        )
        .unwrap();
        let recorded_path = reports.join(format!("{BOOT}.json"));
        fs::write(
            &recorded_path,
            json!({
                "schema": "agentctl-restore-report/v1", "boot_id": BOOT,
                "complete": true, "refused": 1,
                "coordinator": {"name": "coordinator", "action": "revived"},
                "agents": [
                    {"name": "lane", "action": "revived"},
                    {"name": "stuck", "action": "refused", "reason": "no session"},
                ],
                "report": recorded_path,
            })
            .to_string(),
        )
        .unwrap();
        let host = TestHost::new();
        let entries = [RegistryEntry {
            registry: registry.clone(),
            herdr_bin: None,
        }];
        // The minimal already-restored payload (before richer rerun
        // payloads): the aggregate must fall back to the recorded
        // report, replay its rows, and keep the 74 class.
        let runner_path = recorded_path.clone();
        let outcome = restore_teams(&host, &entries, &options(), |_, _, _| {
            Ok(json!({
                "schema": "agentctl-restore-report/v1", "boot_id": BOOT,
                "action": "already-restored", "report": runner_path.clone(),
            }))
        })
        .unwrap();
        assert_eq!(outcome.exit_code, 74, "{}", outcome.report);
        assert_eq!(outcome.report["refused"], 1);
        assert_eq!(
            outcome.report["refused_lanes"],
            json!([{"registry": registry, "name": "stuck"}])
        );
        let rows = outcome.report["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 3, "{}", outcome.report);
        assert!(rows.iter().all(|row| row["replayed"] == true));
        assert_eq!(rows[1]["name"], "lane");
        assert_eq!(rows[1]["action"], "revived");
        assert!(outcome
            .table
            .iter()
            .any(|line| line.contains("lane revived") && line.ends_with("(replayed)")));
        assert_eq!(outcome.report["registries"][0]["status"], "refused");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn certificate_timeout_launches_nothing_for_that_registry_only() {
        let root = scratch("cert-gate");
        let gated = root.join("gated");
        let open = root.join("open");
        for registry in [&gated, &open] {
            fs::create_dir_all(registry).unwrap();
        }
        fs::write(
            gated.join("restore.json"),
            json!({
                "schema": "agentctl-restore/v1",
                "certificate_file": root.join("missing.pem"),
                "certificate_fetch": "example-fetch-command",
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            open.join("restore.json"),
            json!({"schema": "agentctl-restore/v1"}).to_string(),
        )
        .unwrap();
        let host = TestHost::new();
        let entries = [
            RegistryEntry {
                registry: gated.clone(),
                herdr_bin: None,
            },
            RegistryEntry {
                registry: open.clone(),
                herdr_bin: None,
            },
        ];
        let calls = Cell::new(0_u32);
        let outcome = restore_teams(&host, &entries, &options(), |entry, _, _| {
            calls.set(calls.get() + 1);
            assert_eq!(entry.registry, open, "the gated registry must not run");
            Ok(json!({
                "schema": "agentctl-restore-report/v1", "boot_id": BOOT,
                "complete": true, "refused": 0,
                "agents": [{"name": "lane", "action": "revived"}],
            }))
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(outcome.exit_code, 75, "{}", outcome.report);
        let first = &outcome.report["registries"][0];
        assert_eq!(first["status"], "certificate-timeout");
        assert_eq!(first["certificate"]["remedy"], "example-fetch-command");
        assert_eq!(outcome.report["registries"][1]["status"], "complete");
        assert!(outcome
            .table
            .iter()
            .any(|line| line.contains("certificate-timeout")
                && line.contains("example-fetch-command")));
        let _ = fs::remove_dir_all(&root);
    }
}
