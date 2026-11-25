//! Everything `agentctl services` asks of the machine: the user's systemd manager, the processes
//! it runs, and the binaries it would run. Tests replace it with a fake manager.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// What `systemctl show` says about one unit.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnitState {
    /// The unit name, such as `bridge.service`.
    pub id: String,
    /// `loaded`, `not-found`, ...
    pub load_state: String,
    /// `active`, `inactive`, `failed`, `activating`, ...
    pub active_state: String,
    /// The main process, when there is one.
    pub main_pid: Option<u32>,
    /// The unit file systemd loaded.
    pub fragment_path: Option<PathBuf>,
    /// Drop-in files systemd loaded, in order.
    pub drop_in_paths: Vec<PathBuf>,
    /// Whether the unit was created through the API (`systemd-run`).
    pub transient: bool,
    /// The raw `ExecStart` property, used only to decide which units to read in full.
    pub exec_start: String,
}

/// The machine, as seen by `agentctl services`.
pub trait Host {
    /// Every service unit the user's manager knows: loaded ones and installed unit files.
    fn service_units(&self) -> Result<Vec<String>, String>;
    /// `systemctl --user show` for each unit, in the same order.
    fn show(&self, units: &[String]) -> Result<Vec<UnitState>, String>;
    /// `systemctl --user cat UNIT`.
    fn cat(&self, unit: &str) -> Result<String, String>;
    /// `systemctl --user stop UNIT`.
    fn stop(&self, unit: &str) -> Result<(), String>;
    /// `systemctl --user reset-failed UNIT`, whose failure (nothing to reset) is ignored.
    fn reset_failed(&self, unit: &str);
    /// `systemctl --user restart UNIT`.
    fn restart(&self, unit: &str) -> Result<(), String>;
    /// `systemctl --user start UNIT`.
    fn start(&self, unit: &str) -> Result<(), String>;
    /// `systemctl --user daemon-reload`.
    fn daemon_reload(&self) -> Result<(), String>;
    /// `systemd-run --user ARGS...`.
    fn systemd_run(&self, args: &[String]) -> Result<(), String>;
    /// The commit an agentctl binary reports on `--version`; `None` when it names none or does
    /// not run.
    fn binary_commit(&self, binary: &Path) -> Option<String>;
    /// The SHA-256 of a file, in lowercase hexadecimal.
    fn file_sha256(&self, path: &Path) -> Option<String>;
    /// Read a small file.
    fn read_file(&self, path: &Path) -> Option<Vec<u8>>;
    /// The executable a process runs, as `/proc/PID/exe` names it.
    fn process_exe(&self, pid: u32) -> Option<PathBuf>;
    /// The direct children of a process: `Some` when they could be
    /// determined (possibly none), `None` when they could not — an
    /// unreadable /proc — so a caller can say "unknowable" instead of
    /// treating an undetermined child set as an empty one.
    fn process_children(&self, pid: u32) -> Option<Vec<u32>>;
    /// The environment a process started with, from `/proc/PID/environ`.
    fn process_environ(&self, pid: u32) -> Option<Vec<(String, String)>>;
    /// Run a program with an environment and return its exit status and standard output.
    fn run(
        &self,
        program: &Path,
        args: &[String],
        environment: &[(String, String)],
    ) -> Result<(i32, String), String>;
    /// Ask a headless wrapper's control socket for its status.
    fn headless_status(&self, socket: &Path) -> Result<Value, String>;
    /// The user manager's specifier values (#1493): what `%h`, `%u`, `%U`,
    /// `%t`, `%S`, `%C` and `%E` mean for units this host's manager runs.
    /// The default reads this process's manager environment; a fake host
    /// inherits it and units may still name their own `HOME`.
    fn specifier_context(&self) -> super::unit::SpecifierContext {
        system_specifier_context()
    }
    /// The current time.
    fn now(&self) -> SystemTime;
    /// Wait.
    fn sleep(&self, duration: Duration);
}

/// The real machine: `systemctl` and `systemd-run` for the user manager, `/proc`, and files.
pub struct SystemHost {
    /// The systemctl executable.
    pub systemctl: PathBuf,
    /// The systemd-run executable.
    pub systemd_run: PathBuf,
}

/// The `systemctl show` properties [`UnitState`] needs.
const SHOW_PROPERTIES: &str =
    "Id,LoadState,ActiveState,MainPID,FragmentPath,DropInPaths,Transient,ExecStart";

/// The most units passed to one `systemctl show`.
const SHOW_BATCH: usize = 64;

/// How long a program [`Host::run`] starts may take before it is killed.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

fn output_text(output: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if text.is_empty() {
        text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    }
    text
}

impl SystemHost {
    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        // Its own process group: an interrupt typed at the terminal must not stop a systemctl
        // that is halfway through a change.
        let output = Command::new(&self.systemctl)
            .arg("--user")
            .args(args)
            .process_group(0)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("cannot run {}: {error}", self.systemctl.display()))?;
        if !output.status.success() {
            return Err(format!(
                "systemctl --user {} failed ({}): {}",
                args.join(" "),
                output.status,
                output_text(&output)
            ));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| format!("systemctl --user {} printed non-UTF-8", args.join(" ")))
    }
}

/// Parse `systemctl show` output for several units: blocks of `Key=Value` lines, one per unit,
/// separated by blank lines.
#[must_use]
pub fn parse_show(text: &str) -> Vec<UnitState> {
    let mut states = Vec::new();
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut flush = |fields: &mut BTreeMap<String, String>| {
        if fields.is_empty() {
            return;
        }
        let field = |name: &str| fields.get(name).cloned().unwrap_or_default();
        let path = |value: String| (!value.is_empty()).then(|| PathBuf::from(value));
        states.push(UnitState {
            id: field("Id"),
            load_state: field("LoadState"),
            active_state: field("ActiveState"),
            main_pid: field("MainPID").parse().ok().filter(|pid| *pid != 0),
            fragment_path: path(field("FragmentPath")),
            drop_in_paths: field("DropInPaths")
                .split_whitespace()
                .map(PathBuf::from)
                .collect(),
            transient: field("Transient") == "yes",
            exec_start: field("ExecStart"),
        });
        fields.clear();
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut fields);
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            // A unit with several ExecStart commands lists each; keep them all for the filter.
            fields
                .entry(key.to_owned())
                .and_modify(|existing| {
                    existing.push(' ');
                    existing.push_str(value);
                })
                .or_insert_with(|| value.to_owned());
        }
    }
    flush(&mut fields);
    states
}

impl Host for SystemHost {
    fn service_units(&self) -> Result<Vec<String>, String> {
        let mut units = Vec::new();
        let loaded = self.systemctl(&[
            "list-units",
            "--all",
            "--type=service",
            "--plain",
            "--no-legend",
            "--no-pager",
        ])?;
        let files = self.systemctl(&[
            "list-unit-files",
            "--type=service",
            "--plain",
            "--no-legend",
            "--no-pager",
        ])?;
        for line in loaded.lines().chain(files.lines()) {
            let Some(name) = line.split_whitespace().next() else {
                continue;
            };
            // Template units (`name@.service`) run nothing until instantiated.
            if name.ends_with(".service") && !name.ends_with("@.service") {
                units.push(name.to_owned());
            }
        }
        units.sort();
        units.dedup();
        Ok(units)
    }

    fn show(&self, units: &[String]) -> Result<Vec<UnitState>, String> {
        let mut states = Vec::new();
        for batch in units.chunks(SHOW_BATCH) {
            let mut args = vec!["show", "--property", SHOW_PROPERTIES];
            args.extend(batch.iter().map(String::as_str));
            states.extend(parse_show(&self.systemctl(&args)?));
        }
        Ok(states)
    }

    fn cat(&self, unit: &str) -> Result<String, String> {
        self.systemctl(&["cat", "--no-pager", unit])
    }

    fn stop(&self, unit: &str) -> Result<(), String> {
        self.systemctl(&["stop", unit]).map(drop)
    }

    fn reset_failed(&self, unit: &str) {
        let _ = self.systemctl(&["reset-failed", unit]);
    }

    fn restart(&self, unit: &str) -> Result<(), String> {
        self.systemctl(&["restart", unit]).map(drop)
    }

    fn start(&self, unit: &str) -> Result<(), String> {
        self.systemctl(&["start", unit]).map(drop)
    }

    fn daemon_reload(&self) -> Result<(), String> {
        self.systemctl(&["daemon-reload"]).map(drop)
    }

    fn systemd_run(&self, args: &[String]) -> Result<(), String> {
        let output = Command::new(&self.systemd_run)
            .arg("--user")
            .args(args)
            .process_group(0)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("cannot run {}: {error}", self.systemd_run.display()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "systemd-run failed ({}): {}",
                output.status,
                output_text(&output)
            ))
        }
    }

    fn binary_commit(&self, binary: &Path) -> Option<String> {
        let output = Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        parse_version_commit(&String::from_utf8_lossy(&output.stdout))
    }

    fn file_sha256(&self, path: &Path) -> Option<String> {
        let mut file = fs::File::open(path).ok()?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 1 << 16];
        loop {
            let read = file.read(&mut buffer).ok()?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Some(hex(&hasher.finalize()))
    }

    fn read_file(&self, path: &Path) -> Option<Vec<u8>> {
        let file = fs::File::open(path).ok()?;
        let mut bytes = Vec::new();
        file.take(4 << 20).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }

    fn process_exe(&self, pid: u32) -> Option<PathBuf> {
        fs::read_link(format!("/proc/{pid}/exe")).ok()
    }

    fn process_children(&self, pid: u32) -> Option<Vec<u32>> {
        process_children_at(Path::new("/proc"), pid)
    }

    fn process_environ(&self, pid: u32) -> Option<Vec<(String, String)>> {
        let bytes = fs::read(format!("/proc/{pid}/environ")).ok()?;
        Some(
            bytes
                .split(|byte| *byte == 0)
                .filter_map(|entry| {
                    let text = String::from_utf8_lossy(entry);
                    let (name, value) = text.split_once('=')?;
                    Some((name.to_owned(), value.to_owned()))
                })
                .collect(),
        )
    }

    fn run(
        &self,
        program: &Path,
        args: &[String],
        environment: &[(String, String)],
    ) -> Result<(i32, String), String> {
        let mut child = Command::new(program)
            .args(args)
            .envs(environment.iter().map(|(name, value)| (name, value)))
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("cannot run {}: {error}", program.display()))?;
        let mut stdout = child.stdout.take().expect("piped");
        let reader = std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = stdout.read_to_end(&mut text);
            text
        });
        let deadline = std::time::Instant::now() + RUN_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(format!(
                        "{} did not finish within {} s",
                        program.display(),
                        RUN_TIMEOUT.as_secs()
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(error) => return Err(format!("{}: {error}", program.display())),
            }
        };
        let text = reader.join().unwrap_or_default();
        Ok((
            status.code().unwrap_or(-1),
            String::from_utf8_lossy(&text).into_owned(),
        ))
    }

    fn headless_status(&self, socket: &Path) -> Result<Value, String> {
        crate::headless::control::request(socket, &json!({"op": "status"}), Duration::from_secs(10))
            .map_err(|error| error.to_string())
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// The direct children of `pid` under `proc_root` (this, with a fake
/// proc root, is what the tests drive). The task `children` file is the
/// first source; kernels built without `CONFIG_PROC_CHILDREN` do not
/// provide it, so its absence falls back to a parent-pid scan of every
/// `/proc/PID/stat` record — the same fallback the other process readers
/// in this crate use, through the shared stat parser. `None` when
/// neither source can be read: the children could not be determined,
/// which is not the same as a process having none.
#[must_use]
pub fn process_children_at(proc_root: &Path, pid: u32) -> Option<Vec<u32>> {
    let children_file = proc_root
        .join(pid.to_string())
        .join("task")
        .join(pid.to_string())
        .join("children");
    match fs::read_to_string(&children_file) {
        Ok(text) => {
            return Some(
                text.split_whitespace()
                    .filter_map(|child| child.parse().ok())
                    .collect(),
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    let entries = fs::read_dir(proc_root).ok()?;
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let Some(child) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(bytes) = fs::read(entry.path().join("stat")) else {
            continue;
        };
        if agent_queue::identity::parse_process_stat(&bytes, u64::from(child))
            .is_ok_and(|stat| stat.parent_pid == u64::from(pid))
        {
            children.push(child);
        }
    }
    children.sort_unstable();
    Some(children)
}

/// The running user's specifier values, from the environment the user
/// manager itself uses (#1493).
#[must_use]
pub fn system_specifier_context() -> super::unit::SpecifierContext {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let home = absolute("HOME").unwrap_or_else(|| PathBuf::from("/"));
    let uid = fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .and_then(|fields| fields.split_whitespace().next())
                .and_then(|uid| uid.parse::<u32>().ok())
        })
        .unwrap_or(0);
    let runtime_dir =
        absolute("XDG_RUNTIME_DIR").unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")));
    super::unit::SpecifierContext {
        state_dir: absolute("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")),
        cache_dir: absolute("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")),
        config_dir: absolute("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
        home,
        user: std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_default(),
        uid,
        runtime_dir,
    }
}

/// The 40-character commit on the `commit ...` line of `agentctl --version`.
#[must_use]
pub fn parse_version_commit(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let commit = line.strip_prefix("commit ")?.split_whitespace().next()?;
        (commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| commit.to_ascii_lowercase())
    })
}

/// Lowercase hexadecimal.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_show_splits_units_and_reads_fields() {
        let text = "Id=a.service\nLoadState=loaded\nActiveState=active\nMainPID=42\nFragmentPath=/run/a.service\nDropInPaths=/x/a.d/1.conf /x/a.d/2.conf\nTransient=yes\nExecStart={ path=/bin/sh ; argv[]=/bin/sh -c exec agentctl }\n\nId=b.service\nLoadState=not-found\nActiveState=inactive\nMainPID=0\nFragmentPath=\nDropInPaths=\nTransient=no\nExecStart=\n";
        let states = parse_show(text);
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].main_pid, Some(42));
        assert!(states[0].transient);
        assert_eq!(states[0].drop_in_paths.len(), 2);
        assert_eq!(states[1].main_pid, None);
        assert_eq!(states[1].fragment_path, None);
    }

    /// A minimal stat record for `pid` whose parent is `parent`.
    fn stat_record(pid: u32, parent: u32) -> String {
        format!(
            "{pid} (agentctl) S {parent} {pid} 0 -1 4194304 1 2 3 4 13 14 15 16 20 0 1 0 22 0 0"
        )
    }

    fn fake_proc(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agentctl-host-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn children_come_from_the_stat_scan_without_a_children_file() {
        // No CONFIG_PROC_CHILDREN: no task children file anywhere, so
        // the parent-pid stat scan must find the child.
        let root = fake_proc("stat-scan");
        for (pid, parent) in [(100, 1), (101, 100), (102, 1)] {
            let dir = root.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("stat"), stat_record(pid, parent)).unwrap();
        }
        assert_eq!(process_children_at(&root, 100), Some(vec![101]));
        assert_eq!(process_children_at(&root, 102), Some(Vec::new()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_children_file_wins_over_the_stat_scan() {
        let root = fake_proc("children-file");
        let task = root.join("100/task/100");
        fs::create_dir_all(&task).unwrap();
        fs::write(task.join("children"), "102\n").unwrap();
        for (pid, parent) in [(101, 100), (102, 1)] {
            let dir = root.join(pid.to_string());
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("stat"), stat_record(pid, parent)).unwrap();
        }
        assert_eq!(process_children_at(&root, 100), Some(vec![102]));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreadable_proc_cannot_determine_children() {
        let root = fake_proc("unreadable");
        let missing = root.join("no-such-proc");
        assert_eq!(process_children_at(&missing, 100), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn version_commit_needs_a_full_hash() {
        let commit = "92f81d814902e264297e5ad015f486b90235a2fe";
        assert_eq!(
            parse_version_commit(&format!("agentctl 0.1.0\ncommit {commit}\n")),
            Some(commit.to_owned())
        );
        assert_eq!(
            parse_version_commit(&format!("agentctl 0.1.0\ncommit {commit} (dirty)\n")),
            Some(commit.to_owned())
        );
        assert_eq!(parse_version_commit("agentctl 0.1.0\n"), None);
        assert_eq!(parse_version_commit("commit abc\n"), None);
    }
}
