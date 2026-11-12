//! Guarded single-PID signalling (#1420 guarded-kill-pid).
//!
//! The 2026-11-11 outage began when an agent sent SIGTERM to a PID it took for a
//! stale script's parent; the PID was the host's `systemd --user` manager and
//! every user unit stopped. This module makes the safe kill mechanical: a PID may
//! be signalled only when its command line and start time still match what the
//! operator recorded (so a reused PID is refused), and never when it is the user
//! manager, a `user@UID.service` main process, the Herdr server, the main PID
//! of a systemd unit the caller's team does not own, or a process the caller's
//! team cannot prove it owns. Ownership is an allowlist: the process's working
//! directory must sit under the caller's team root (the directory holding the
//! registry, which covers its slots), or it must be the main PID of a unit the
//! team's own registry configuration names. Everything else is refused.
//!
//! All system access goes through the [`ProcView`], [`UnitView`] and [`Killer`]
//! traits, so tests replace `/proc` and `systemctl` with fixtures and never signal a real process.
//! The real killer re-reads the process identity and, on Linux, signals through a
//! pidfd pinned before the recheck, so the signal can only reach the exact
//! process that was checked. This is the Rust edition of `py/agentctl/killpid.py`;
//! the refusal reason strings are identical in both editions.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Signal names `kill-pid` accepts; anything else is a usage error.
pub const SIGNALS: [(&str, i32); 8] = [
    ("TERM", libc::SIGTERM),
    ("KILL", libc::SIGKILL),
    ("INT", libc::SIGINT),
    ("HUP", libc::SIGHUP),
    ("QUIT", libc::SIGQUIT),
    ("USR1", libc::SIGUSR1),
    ("USR2", libc::SIGUSR2),
    ("CONT", libc::SIGCONT),
];

const MAX_CMDLINE_BYTES: usize = 256 << 10;
const MAX_UNITS: usize = 4096;
const LOG_NAME: &str = "kill-pid.jsonl";

/// One process as the guard needs to see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    /// Process identifier.
    pub pid: u64,
    /// Argument vector.
    pub argv: Vec<String>,
    /// Start time in clock ticks (field 22 of `/proc/PID/stat`).
    pub starttime: u64,
    /// Working directory, when readable.
    pub cwd: Option<String>,
    /// Contents of `/proc/PID/cgroup`.
    pub cgroup: String,
    /// Kernel command name.
    pub comm: String,
}

impl ProcessInfo {
    /// The argument vector joined the way an operator would record it.
    #[must_use]
    pub fn cmdline(&self) -> String {
        self.argv.join(" ")
    }
}

/// One systemd user unit and its main PID (0 when it has none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitInfo {
    /// Unit name, for example `chat-bridge.service`.
    pub name: String,
    /// Main process identifier, or 0.
    pub main_pid: u64,
}

/// What the caller's team owns: its root directory and its unit names.
#[derive(Clone, Debug)]
pub struct OwnedContext {
    /// Directory holding the registry; the team's slots live under it too.
    pub team_root: PathBuf,
    /// Unit names the team's registry configuration declares as its own.
    pub owned_units: BTreeSet<String>,
    /// This process's own PID, never a target.
    pub self_pid: u64,
}

/// A kill that was performed and logged.
#[derive(Clone, Debug)]
pub struct KillReport {
    /// Signalled process.
    pub pid: u64,
    /// Signal name that was sent.
    pub signal_name: String,
    /// Command line that was verified.
    pub cmdline: String,
    /// Start time that was verified.
    pub starttime: u64,
    /// Working directory that was verified.
    pub cwd: Option<String>,
    /// The team's kill log the record was appended to.
    pub log_path: PathBuf,
}

/// The guard refused to signal the PID; `reason` says why, mechanically.
#[derive(Clone, Debug)]
pub struct KillRefused {
    /// Stable refusal reason, shared with the Python edition.
    pub reason: &'static str,
    /// Human detail naming the process and the expectation it failed.
    pub detail: String,
}

impl std::fmt::Display for KillRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.detail)
    }
}

impl std::error::Error for KillRefused {}

fn refused(reason: &'static str, detail: String) -> KillRefused {
    KillRefused { reason, detail }
}

/// Read access to one process's identity, replaced by test doubles.
pub trait ProcView {
    /// The process, or None when no such process can be read.
    fn process(&self, pid: u64) -> Option<ProcessInfo>;
}

/// Read access to the user manager's units, replaced by test doubles.
pub trait UnitView {
    /// Every user unit with its main PID.
    fn units(&self) -> Vec<UnitInfo>;
}

/// The one signal send, replaced by test doubles so no real process is signalled.
pub trait Killer {
    /// Signal `pid`, re-verifying it still is `expected` first.
    ///
    /// # Errors
    /// Returns [`KillRefused`] with reason `changed-before-signal` when the
    /// process changed between the check and the signal.
    fn send(
        &self,
        pid: u64,
        signum: i32,
        view: &dyn ProcView,
        expected: &ProcessInfo,
    ) -> Result<(), KillRefused>;
}

/// The real view, read from a procfs root (`/proc` unless a fixture root is given).
pub struct RealProcView {
    root: PathBuf,
}

impl RealProcView {
    /// Read processes from `root`.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Default for RealProcView {
    fn default() -> Self {
        Self::new(PathBuf::from("/proc"))
    }
}

impl ProcView for RealProcView {
    fn process(&self, pid: u64) -> Option<ProcessInfo> {
        let base = self.root.join(pid.to_string());
        let stat_raw = fs::read(base.join("stat")).ok()?;
        if stat_raw.len() > 8192 {
            return None;
        }
        let parsed = agent_queue::identity::parse_process_stat(&stat_raw, pid).ok()?;
        let raw = fs::read(base.join("cmdline")).unwrap_or_default();
        let argv: Vec<String> = if raw.is_empty() || raw.len() > MAX_CMDLINE_BYTES {
            Vec::new()
        } else {
            raw.split(|byte| *byte == 0)
                .filter(|item| !item.is_empty())
                .map(|item| String::from_utf8_lossy(item).into_owned())
                .collect()
        };
        let cwd = fs::read_link(base.join("cwd"))
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
        let cgroup = fs::read_to_string(base.join("cgroup")).unwrap_or_default();
        let comm = comm_of(&stat_raw);
        Some(ProcessInfo {
            pid,
            argv,
            starttime: parsed.starttime_ticks,
            cwd,
            cgroup,
            comm,
        })
    }
}

fn comm_of(stat_raw: &[u8]) -> String {
    let opening = stat_raw.windows(2).position(|window| window == b" (");
    let closing = stat_raw.iter().rposition(|byte| *byte == b')');
    match (opening, closing) {
        (Some(open), Some(close)) if close > open + 1 => {
            String::from_utf8_lossy(&stat_raw[open + 2..close]).into_owned()
        }
        _ => String::new(),
    }
}

/// The real view: `systemctl --user show` for every loaded service.
pub struct RealUnitView {
    systemctl: PathBuf,
}

impl RealUnitView {
    /// Query units through the `systemctl` executable at `systemctl`.
    #[must_use]
    pub fn new(systemctl: PathBuf) -> Self {
        Self { systemctl }
    }
}

impl Default for RealUnitView {
    fn default() -> Self {
        Self::new(PathBuf::from("systemctl"))
    }
}

impl UnitView for RealUnitView {
    fn units(&self) -> Vec<UnitInfo> {
        let output = Command::new(&self.systemctl)
            .args([
                "--user",
                "show",
                "--all",
                "--property=Id,MainPID",
                "--type=service",
            ])
            .stdin(Stdio::null())
            .output();
        let Ok(output) = output else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut units = Vec::new();
        let mut name = String::new();
        let mut main_pid: u64 = 0;
        for line in text.lines().chain(std::iter::once("")) {
            if line.trim().is_empty() {
                if !name.is_empty() {
                    units.push(UnitInfo {
                        name: std::mem::take(&mut name),
                        main_pid,
                    });
                    if units.len() > MAX_UNITS {
                        return Vec::new();
                    }
                }
                main_pid = 0;
            } else if let Some(value) = line.strip_prefix("Id=") {
                name = value.to_owned();
            } else if let Some(value) = line.strip_prefix("MainPID=") {
                main_pid = value.parse().unwrap_or(0);
            }
        }
        units
    }
}

/// Signal through a pidfd pinned before the identity recheck, when able.
pub struct RealKiller;

impl Killer for RealKiller {
    fn send(
        &self,
        pid: u64,
        signum: i32,
        view: &dyn ProcView,
        expected: &ProcessInfo,
    ) -> Result<(), KillRefused> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            // SAFETY: pidfd_open takes scalar arguments and returns a fresh descriptor.
            let descriptor =
                unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0_u32) };
            if descriptor >= 0 {
                // SAFETY: the descriptor came from pidfd_open just above and is closed once.
                let file = unsafe { std::os::fd::OwnedFd::from_raw_fd(descriptor as i32) };
                let current = view.process(pid);
                if !matches!(&current, Some(now) if same_identity(now, expected)) {
                    return Err(refused(
                        "changed-before-signal",
                        format!(
                            "pid {pid} changed between the check and the signal; not signalling"
                        ),
                    ));
                }
                // SAFETY: pidfd addresses the pinned process identity; null siginfo is the
                // documented way to send a plain signal.
                let rc = unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        file.as_raw_fd(),
                        signum,
                        std::ptr::null::<libc::siginfo_t>(),
                        0_u32,
                    )
                };
                if rc != 0 {
                    return Err(refused(
                        "signal-failed",
                        format!(
                            "pidfd signal to pid {pid} failed: {}",
                            std::io::Error::last_os_error()
                        ),
                    ));
                }
                return Ok(());
            }
        }
        let current = view.process(pid);
        if !matches!(&current, Some(now) if same_identity(now, expected)) {
            return Err(refused(
                "changed-before-signal",
                format!("pid {pid} changed between the check and the signal; not signalling"),
            ));
        }
        // SAFETY: kill with a scalar pid and signal; the identity was just re-verified.
        let rc = unsafe { libc::kill(pid as libc::pid_t, signum) };
        if rc != 0 {
            return Err(refused(
                "signal-failed",
                format!(
                    "signal to pid {pid} failed: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        Ok(())
    }
}

fn same_identity(left: &ProcessInfo, right: &ProcessInfo) -> bool {
    left.pid == right.pid && left.starttime == right.starttime && left.cmdline() == right.cmdline()
}

/// Unit names the team's own registry configuration declares as its own.
#[must_use]
pub fn load_owned_units(registry: &Path) -> BTreeSet<String> {
    let mut owned = BTreeSet::new();
    for filename in ["restore.json", "watch.json"] {
        let Ok(text) = fs::read_to_string(registry.join(filename)) else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for key in ["units", "bridges"] {
            if let Some(items) = document.get(key).and_then(Value::as_array) {
                for item in items {
                    if let Some(name) = item.as_str().filter(|name| !name.is_empty()) {
                        owned.insert(name.to_owned());
                    }
                }
            }
        }
    }
    owned
}

fn is_user_manager(process: &ProcessInfo) -> bool {
    let argv0 = process.argv.first().map(String::as_str).unwrap_or("");
    let base = Path::new(argv0)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    (base == "systemd" && process.argv.iter().any(|arg| arg == "--user"))
        || process.cmdline().contains("systemd --user")
}

fn is_herdr_server(process: &ProcessInfo) -> bool {
    let Some(argv0) = process.argv.first() else {
        return process.comm == "herdr";
    };
    let base = Path::new(argv0)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    base == "herdr" && process.argv.iter().skip(1).any(|arg| arg == "server")
}

fn other_team_root(cwd: &str, team_root: &Path) -> bool {
    let root = team_root.to_string_lossy();
    if cwd == root || cwd.starts_with(&format!("{root}/")) {
        return false;
    }
    let padded = format!("{cwd}/");
    padded.contains("/.agentctl/") || padded.contains("/worktrees/slots/")
}

/// Check one snapshot against every refusal; return it when it may be killed.
///
/// # Errors
/// Returns [`KillRefused`] naming the first refusal that applies.
pub fn evaluate(
    process: Option<ProcessInfo>,
    pid: u64,
    expect_cmdline: &str,
    expect_start: u64,
    units: &[UnitInfo],
    context: &OwnedContext,
) -> Result<ProcessInfo, KillRefused> {
    let Some(process) = process else {
        return Err(refused(
            "no-such-process",
            format!("no readable process with pid {pid}"),
        ));
    };
    if pid <= 1 || pid == context.self_pid {
        return Err(refused(
            "protected-pid",
            format!("pid {pid} is never a kill-pid target"),
        ));
    }
    if !process.cmdline().contains(expect_cmdline) {
        return Err(refused(
            "cmdline-mismatch",
            format!(
                "pid {pid} runs {:?}, which does not contain the expected {expect_cmdline:?}",
                process.cmdline()
            ),
        ));
    }
    if process.starttime != expect_start {
        return Err(refused(
            "starttime-mismatch",
            format!(
                "pid {pid} started at tick {}, not {expect_start}; the PID was probably reused",
                process.starttime
            ),
        ));
    }
    if is_user_manager(&process) {
        return Err(refused(
            "user-manager",
            format!("pid {pid} is the systemd --user manager; signalling it stops every user unit"),
        ));
    }
    for unit in units {
        if unit.main_pid == pid && unit.name.starts_with("user@") {
            return Err(refused(
                "user-service",
                format!(
                    "pid {pid} is the main process of {}; signalling it stops every user unit",
                    unit.name
                ),
            ));
        }
    }
    if is_herdr_server(&process) {
        return Err(refused(
            "herdr-server",
            format!("pid {pid} is the Herdr server; signalling it drops every agent's terminal"),
        ));
    }
    for unit in units {
        if unit.main_pid == pid && !context.owned_units.contains(&unit.name) {
            return Err(refused(
                "foreign-unit-main-pid",
                format!(
                    "pid {pid} is the main process of unit {}, which this team does not own",
                    unit.name
                ),
            ));
        }
    }
    let cwd = process.cwd.clone();
    let owned_unit = units.iter().any(|unit| unit.main_pid == pid);
    if let Some(dir) = &cwd {
        let root = context.team_root.to_string_lossy().into_owned();
        if *dir == root || dir.starts_with(&format!("{root}/")) {
            return Ok(process);
        }
    }
    if owned_unit {
        return Ok(process);
    }
    if let Some(dir) = &cwd {
        if other_team_root(dir, &context.team_root) {
            return Err(refused(
                "other-team",
                format!(
                    "pid {pid} works in {dir}, which belongs to another team's registry or slots"
                ),
            ));
        }
    }
    Err(refused(
        "not-owned",
        format!(
            "pid {pid} works in {:?}, outside this team's root {}; kill-pid only signals processes the caller's team can prove it owns",
            cwd,
            context.team_root.display()
        ),
    ))
}

/// Arguments of one guarded kill.
pub struct KillRequest<'a> {
    /// The single process to signal.
    pub pid: u64,
    /// Text the command line must contain.
    pub expect_cmdline: &'a str,
    /// Expected start time in clock ticks.
    pub expect_start: u64,
    /// Signal name, one of [`SIGNALS`].
    pub signal_name: &'a str,
    /// The caller's registry; its parent is the team root and it holds the log.
    pub registry: &'a Path,
    /// This process's own PID (never a target); injectable for tests.
    pub self_pid: u64,
    /// Kill time as Unix seconds; injectable for tests.
    pub now: Option<f64>,
}

/// Guard, signal and log one PID, or refuse.
///
/// # Errors
/// Returns [`KillRefused`] when any check refuses, or the send fails.
pub fn guarded_kill(
    request: &KillRequest<'_>,
    proc: &dyn ProcView,
    unit_view: &dyn UnitView,
    killer: &dyn Killer,
) -> Result<KillReport, KillRefused> {
    let Some((_, signum)) = SIGNALS
        .iter()
        .find(|(name, _)| *name == request.signal_name)
    else {
        return Err(refused(
            "bad-signal",
            format!(
                "unknown signal {:?}; use one of TERM, KILL, INT, HUP, QUIT, USR1, USR2, CONT",
                request.signal_name
            ),
        ));
    };
    if request.expect_cmdline.is_empty() {
        return Err(refused(
            "bad-expectation",
            "--expect-cmdline must not be empty".to_owned(),
        ));
    }
    let registry = request
        .registry
        .canonicalize()
        .unwrap_or_else(|_| request.registry.to_path_buf());
    let context = OwnedContext {
        team_root: registry
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/")),
        owned_units: load_owned_units(&registry),
        self_pid: request.self_pid,
    };
    let units = unit_view.units();
    let checked = evaluate(
        proc.process(request.pid),
        request.pid,
        request.expect_cmdline,
        request.expect_start,
        &units,
        &context,
    )?;
    killer.send(request.pid, *signum, proc, &checked)?;
    let stamp = request.now.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0)
    });
    let actor = std::env::var("AGENTCTL_AGENT").unwrap_or_else(|_| "unknown".to_owned());
    let record = json!({
        "schema": "agentctl-kill-pid/v1",
        "time": stamp,
        "pid": request.pid,
        "signal": request.signal_name,
        "cmdline": checked.cmdline(),
        "starttime": checked.starttime,
        "cwd": checked.cwd,
        "actor": actor,
    });
    let log_path = registry.join(LOG_NAME);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| {
            refused(
                "log-failed",
                format!("cannot open kill log {}: {error}", log_path.display()),
            )
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    writeln!(file, "{record}").map_err(|error| {
        refused(
            "log-failed",
            format!("cannot write kill log {}: {error}", log_path.display()),
        )
    })?;
    Ok(KillReport {
        pid: request.pid,
        signal_name: request.signal_name.to_owned(),
        cmdline: checked.cmdline(),
        starttime: checked.starttime,
        cwd: checked.cwd,
        log_path,
    })
}

/// The CLI entry: real /proc, real systemctl, real (pidfd) signal.
///
/// # Errors
/// Returns [`KillRefused`] when any check refuses, or the send fails.
pub fn run_kill_pid(
    pid: u64,
    expect_cmdline: &str,
    expect_start: u64,
    signal_name: &str,
    registry: &Path,
) -> Result<KillReport, KillRefused> {
    let request = KillRequest {
        pid,
        expect_cmdline,
        expect_start,
        signal_name,
        registry,
        self_pid: u64::from(std::process::id()),
        now: None,
    };
    guarded_kill(
        &request,
        &RealProcView::default(),
        &RealUnitView::default(),
        &RealKiller,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    struct FakeProc {
        processes: RefCell<BTreeMap<u64, ProcessInfo>>,
    }

    impl ProcView for FakeProc {
        fn process(&self, pid: u64) -> Option<ProcessInfo> {
            self.processes.borrow().get(&pid).cloned()
        }
    }

    struct FakeUnits(Vec<UnitInfo>);

    impl UnitView for FakeUnits {
        fn units(&self) -> Vec<UnitInfo> {
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct FakeKiller {
        sent: RefCell<Vec<(u64, i32)>>,
        change_on_send: RefCell<Option<ProcessInfo>>,
    }

    impl Killer for FakeKiller {
        fn send(
            &self,
            pid: u64,
            signum: i32,
            view: &dyn ProcView,
            expected: &ProcessInfo,
        ) -> Result<(), KillRefused> {
            if let Some(changed) = self.change_on_send.borrow().clone() {
                let _ = view;
                let _ = changed;
                return Err(refused(
                    "changed-before-signal",
                    format!("pid {pid} changed between the check and the signal; not signalling"),
                ));
            }
            let _ = expected;
            self.sent.borrow_mut().push((pid, signum));
            Ok(())
        }
    }

    fn process(pid: u64, argv: &[&str], starttime: u64, cwd: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            argv: argv.iter().map(|item| (*item).to_owned()).collect(),
            starttime,
            cwd: cwd.map(str::to_owned),
            cgroup: String::new(),
            comm: argv.first().copied().unwrap_or("").to_owned(),
        }
    }

    fn temp_registry() -> (tempfile_dir::Dir, PathBuf) {
        let dir = tempfile_dir::Dir::new();
        let registry = dir.path().join("team").join(".agentctl");
        fs::create_dir_all(&registry).expect("registry");
        (dir, registry)
    }

    mod tempfile_dir {
        use std::path::PathBuf;

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "agentctl-kill-pid-test-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                ));
                std::fs::create_dir_all(&path).expect("temp dir");
                Self(path)
            }

            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn run(
        registry: &Path,
        proc: &FakeProc,
        units: Vec<UnitInfo>,
        killer: &FakeKiller,
        pid: u64,
        expect_cmdline: &str,
        expect_start: u64,
    ) -> Result<KillReport, KillRefused> {
        let request = KillRequest {
            pid,
            expect_cmdline,
            expect_start,
            signal_name: "TERM",
            registry,
            self_pid: 999_999,
            now: Some(1_700_000_000.0),
        };
        guarded_kill(&request, proc, &FakeUnits(units), killer)
    }

    #[test]
    fn owned_child_is_signalled_and_logged() {
        let (_dir, registry) = temp_registry();
        let team_root = registry.parent().expect("team root").to_path_buf();
        let child = process(
            4242,
            &["sleep", "60"],
            111,
            Some(&team_root.to_string_lossy()),
        );
        let proc = FakeProc {
            processes: RefCell::new(BTreeMap::from([(4242, child)])),
        };
        let killer = FakeKiller::default();
        let report = run(&registry, &proc, Vec::new(), &killer, 4242, "sleep 60", 111)
            .expect("owned child must be signalled");
        assert_eq!(report.pid, 4242);
        assert_eq!(killer.sent.borrow().as_slice(), [(4242, libc::SIGTERM)]);
        let log = fs::read_to_string(registry.join(LOG_NAME)).expect("log");
        assert!(log.contains("\"pid\":4242"), "{log}");
        assert!(log.contains("agentctl-kill-pid/v1"), "{log}");
    }

    #[test]
    fn refusals_cover_each_hard_case() {
        let (_dir, registry) = temp_registry();
        let team_root = registry
            .parent()
            .expect("team root")
            .to_string_lossy()
            .into_owned();
        let cases: Vec<(ProcessInfo, Vec<UnitInfo>, &str, u64, &str)> = vec![
            (
                process(
                    4242,
                    &["/usr/lib/systemd/systemd", "--user"],
                    111,
                    Some(&team_root),
                ),
                Vec::new(),
                "systemd --user",
                111,
                "user-manager",
            ),
            (
                process(
                    4243,
                    &["/usr/lib/systemd/systemd", "--user"],
                    111,
                    Some(&team_root),
                ),
                vec![UnitInfo {
                    name: "user@1000.service".to_owned(),
                    main_pid: 4243,
                }],
                "systemd --user",
                111,
                "user-manager",
            ),
            (
                process(4244, &["herdr", "server"], 111, Some(&team_root)),
                Vec::new(),
                "herdr server",
                111,
                "herdr-server",
            ),
            (
                process(4245, &["sleep", "60"], 111, Some(&team_root)),
                vec![UnitInfo {
                    name: "other.service".to_owned(),
                    main_pid: 4245,
                }],
                "sleep 60",
                111,
                "foreign-unit-main-pid",
            ),
            (
                process(
                    4246,
                    &["sleep", "60"],
                    111,
                    Some("/elsewhere/team/.agentctl/agents/x"),
                ),
                Vec::new(),
                "sleep 60",
                111,
                "other-team",
            ),
            (
                process(4247, &["sleep", "60"], 111, Some("/tmp")),
                Vec::new(),
                "sleep 60",
                111,
                "not-owned",
            ),
            (
                process(4248, &["sleep", "60"], 111, Some(&team_root)),
                Vec::new(),
                "sleep 60",
                999,
                "starttime-mismatch",
            ),
            (
                process(4249, &["sleep", "60"], 111, Some(&team_root)),
                Vec::new(),
                "other cmdline",
                111,
                "cmdline-mismatch",
            ),
        ];
        for (info, units, expect, start, reason) in cases {
            let pid = info.pid;
            let proc = FakeProc {
                processes: RefCell::new(BTreeMap::from([(pid, info)])),
            };
            let killer = FakeKiller::default();
            let error = run(&registry, &proc, units, &killer, pid, expect, start)
                .expect_err("case must be refused");
            assert_eq!(error.reason, reason, "pid {pid}");
            assert!(
                killer.sent.borrow().is_empty(),
                "pid {pid} must not be signalled"
            );
        }
    }

    #[test]
    fn missing_process_and_pid_one_are_refused() {
        let (_dir, registry) = temp_registry();
        let proc = FakeProc {
            processes: RefCell::new(BTreeMap::new()),
        };
        let killer = FakeKiller::default();
        let error = run(&registry, &proc, Vec::new(), &killer, 4242, "sleep", 1)
            .expect_err("missing process must be refused");
        assert_eq!(error.reason, "no-such-process");
        let team_root = registry
            .parent()
            .expect("team root")
            .to_string_lossy()
            .into_owned();
        let init = process(1, &["sleep", "60"], 111, Some(&team_root));
        let proc = FakeProc {
            processes: RefCell::new(BTreeMap::from([(1, init)])),
        };
        let error = run(&registry, &proc, Vec::new(), &killer, 1, "sleep 60", 111)
            .expect_err("pid 1 must be refused");
        assert_eq!(error.reason, "protected-pid");
    }
}
