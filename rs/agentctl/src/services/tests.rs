//! `agentctl services` against a fake user manager: transient units made and remade through a
//! `systemd-run` emulation, unit files with drop-ins on disk, processes, and health answers.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::discover::{discover, Kind, OptionTables};
use super::host::{Host, UnitState};
use super::refresh::{definition, refresh, RefreshOptions, DROP_IN};
use super::snapshot::{self, Snapshot};
use super::unit::{quote_word, shell_words, split_words, ExecCommand, UnitText};

const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";

struct Tables;

impl OptionTables for Tables {
    fn value_options(&self, path: &[&str]) -> HashSet<String> {
        let mut names: HashSet<String> = ["registry", "herdr-bin"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let extra: &[&str] = match path {
            ["chat", "run"] | ["chat"] => &["bridge-state"],
            ["watch-agents"] => &["state-dir", "stall-minutes"],
            ["run-headless"] => &["approval-timeout"],
            _ => &[],
        };
        names.extend(extra.iter().map(|name| (*name).to_owned()));
        names
    }
}

#[derive(Clone, Debug)]
struct FakeUnit {
    transient: bool,
    /// The transient file, or the unit file's own text.
    text: String,
    /// The unit file path (transient: under /run).
    fragment: PathBuf,
    /// The text systemd loaded: unit text plus drop-ins at the last reload.
    loaded: String,
    active: String,
    pid: Option<u32>,
}

struct Fake {
    units: RefCell<BTreeMap<String, FakeUnit>>,
    exes: RefCell<BTreeMap<u32, PathBuf>>,
    environs: RefCell<BTreeMap<u32, Vec<(String, String)>>>,
    /// A stopped transient unit stays loaded, as when another unit refers to it.
    sticky: Cell<bool>,
    /// `chat health` says DOWN.
    bridge_down: Cell<bool>,
    commits: RefCell<BTreeMap<PathBuf, String>>,
    next_pid: Cell<u32>,
    clock: Cell<SystemTime>,
    drop_in_dir: PathBuf,
    /// A process started from this binary fails at once.
    crashing: RefCell<Option<PathBuf>>,
    /// systemd-run refuses a property containing this text.
    refuse_property: RefCell<Option<String>>,
    /// Every systemd-run and systemctl call, for assertions.
    calls: RefCell<Vec<String>>,
}

const UNIT_KEYS: [&str; 6] = [
    "Description",
    "After",
    "Wants",
    "Requires",
    "Before",
    "BindsTo",
];

impl Fake {
    fn new(drop_in_dir: PathBuf) -> Self {
        Self {
            units: RefCell::default(),
            exes: RefCell::default(),
            environs: RefCell::default(),
            sticky: Cell::new(false),
            bridge_down: Cell::new(false),
            commits: RefCell::default(),
            next_pid: Cell::new(100),
            clock: Cell::new(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            drop_in_dir,
            crashing: RefCell::new(None),
            refuse_property: RefCell::new(None),
            calls: RefCell::default(),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    /// The program a command really runs: argv[0], or the command of `sh -c "exec X ..."`.
    fn program(argv: &[String]) -> PathBuf {
        if argv[0].ends_with("/sh") && argv.len() > 2 {
            let words = shell_words(&argv[2]).unwrap();
            let command = words.iter().find(|word| word.text != "exec").unwrap();
            return PathBuf::from(&command.text);
        }
        PathBuf::from(&argv[0])
    }

    fn start(&self, name: &str) {
        let mut units = self.units.borrow_mut();
        let unit = units.get_mut(name).expect("unit");
        let parsed = UnitText::parse(&unit.loaded);
        let exec = ExecCommand::parse(parsed.last("Service", "ExecStart").unwrap()).unwrap();
        let mut argv = exec.argv;
        if unit.transient {
            for word in &mut argv {
                *word = word.replace("%%", "%");
            }
        }
        let program = Self::program(&argv);
        if self.crashing.borrow().as_deref() == Some(program.as_path()) {
            unit.active = "failed".into();
            unit.pid = None;
            return;
        }
        let pid = self.next_pid.get();
        self.next_pid.set(pid + 1);
        self.exes.borrow_mut().insert(pid, program);
        let environment = parsed
            .environment()
            .unwrap()
            .into_iter()
            .map(|(name, value)| (name, value.replace("%%", "%")))
            .collect();
        self.environs.borrow_mut().insert(pid, environment);
        unit.active = "active".into();
        unit.pid = Some(pid);
    }

    fn loaded_text(&self, name: &str, unit: &FakeUnit) -> String {
        let mut text = format!("# {}\n{}", unit.fragment.display(), unit.text);
        let directory = self.drop_in_dir.join(format!("{name}.d"));
        let mut files: Vec<PathBuf> = fs::read_dir(&directory)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        files.sort();
        for file in files {
            if file
                .extension()
                .is_some_and(|extension| extension == "conf")
            {
                text.push_str(&format!(
                    "\n# {}\n{}",
                    file.display(),
                    fs::read_to_string(&file).unwrap()
                ));
            }
        }
        text
    }

    fn add_unit_file(&self, name: &str, text: &str) {
        let mut unit = FakeUnit {
            transient: false,
            text: text.to_owned(),
            fragment: PathBuf::from(format!("/home/u/.config/systemd/user/{name}")),
            loaded: String::new(),
            active: "inactive".into(),
            pid: None,
        };
        unit.loaded = self.loaded_text(name, &unit);
        self.units.borrow_mut().insert(name.to_owned(), unit);
        Fake::start(self, name);
    }

    fn service_text(&self, name: &str) -> String {
        self.units.borrow()[name].loaded.clone()
    }

    fn pid(&self, name: &str) -> Option<u32> {
        self.units.borrow().get(name).and_then(|unit| unit.pid)
    }
}

impl Host for Fake {
    fn service_units(&self) -> Result<Vec<String>, String> {
        Ok(self.units.borrow().keys().cloned().collect())
    }

    fn show(&self, units: &[String]) -> Result<Vec<UnitState>, String> {
        let known = self.units.borrow();
        Ok(units
            .iter()
            .map(|name| match known.get(name) {
                Some(unit) => {
                    let parsed = UnitText::parse(&unit.loaded);
                    UnitState {
                        id: name.clone(),
                        load_state: "loaded".into(),
                        active_state: unit.active.clone(),
                        main_pid: unit.pid,
                        fragment_path: Some(unit.fragment.clone()),
                        drop_in_paths: Vec::new(),
                        transient: unit.transient,
                        exec_start: parsed.effective("Service", "ExecStart").join(" "),
                    }
                }
                None => UnitState {
                    id: name.clone(),
                    load_state: "not-found".into(),
                    active_state: "inactive".into(),
                    ..UnitState::default()
                },
            })
            .collect())
    }

    fn cat(&self, unit: &str) -> Result<String, String> {
        self.units
            .borrow()
            .get(unit)
            .map(|unit| unit.loaded.clone())
            .ok_or_else(|| format!("No files found for {unit}."))
    }

    fn stop(&self, unit: &str) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("stop {unit}"));
        let mut units = self.units.borrow_mut();
        let transient = units.get(unit).is_some_and(|unit| unit.transient);
        if transient && !self.sticky.get() {
            units.remove(unit);
        } else if let Some(entry) = units.get_mut(unit) {
            entry.active = "inactive".into();
            entry.pid = None;
        }
        Ok(())
    }

    fn reset_failed(&self, unit: &str) {
        let mut units = self.units.borrow_mut();
        if units
            .get(unit)
            .is_some_and(|entry| entry.transient && entry.active == "failed")
        {
            units.remove(unit);
        }
    }

    fn restart(&self, unit: &str) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("restart {unit}"));
        Fake::start(self, unit);
        Ok(())
    }

    fn start(&self, unit: &str) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("start {unit}"));
        let active = self
            .units
            .borrow()
            .get(unit)
            .ok_or_else(|| format!("Unit {unit} not found."))?
            .active
            .clone();
        if active != "active" {
            Fake::start(self, unit);
        }
        Ok(())
    }

    fn daemon_reload(&self) -> Result<(), String> {
        self.calls.borrow_mut().push("daemon-reload".into());
        let names: Vec<String> = self.units.borrow().keys().cloned().collect();
        for name in names {
            let unit = self.units.borrow()[&name].clone();
            if !unit.transient {
                let loaded = self.loaded_text(&name, &unit);
                self.units.borrow_mut().get_mut(&name).unwrap().loaded = loaded;
            }
        }
        Ok(())
    }

    fn systemd_run(&self, args: &[String]) -> Result<(), String> {
        self.calls
            .borrow_mut()
            .push(format!("systemd-run {}", args.join(" ")));
        let mut name = None;
        let mut description = None;
        let mut unit_lines = Vec::new();
        let mut service_lines = Vec::new();
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            if let Some(value) = arg.strip_prefix("--unit=") {
                name = Some(value.to_owned());
            } else if let Some(value) = arg.strip_prefix("--description=") {
                description = Some(value.to_owned());
            } else if arg == "-p" {
                index += 1;
                let property = &args[index];
                if let Some(refused) = self.refuse_property.borrow().as_deref() {
                    if property.contains(refused) {
                        return Err(format!("Unknown assignment: {property}"));
                    }
                }
                let (key, value) = property.split_once('=').unwrap();
                let value = if key == "Environment" {
                    split_words(value)?
                        .iter()
                        .map(|word| quote_word(&word.replace('%', "%%")))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    value.replace('%', "%%")
                };
                let line = format!("{key}={value}");
                if UNIT_KEYS.contains(&key) {
                    unit_lines.push(line);
                } else {
                    service_lines.push(line);
                }
            } else if arg == "--" {
                index += 1;
                break;
            }
            index += 1;
        }
        let argv: Vec<String> = args[index..].to_vec();
        let name = name.ok_or("no --unit")?;
        if self.units.borrow().contains_key(&name) {
            self.calls
                .borrow_mut()
                .push(format!("refused: {name} already loaded"));
            return Err(format!("Unit {name} was already loaded"));
        }
        let mut text = String::from(
            "# This is a transient unit file, created programmatically via the systemd API. Do not edit.\n[Unit]\n",
        );
        text.push_str(&format!(
            "Description={}\n",
            description
                .unwrap_or_else(|| format!("[systemd-run] {}", argv.join(" ")))
                .replace('%', "%%")
        ));
        for line in unit_lines {
            text.push_str(&format!("{line}\n"));
        }
        text.push_str("\n[Service]\n");
        for line in service_lines {
            text.push_str(&format!("{line}\n"));
        }
        let exec = ExecCommand {
            prefix: String::new(),
            argv: argv.iter().map(|word| word.replace('%', "%%")).collect(),
        };
        text.push_str(&format!("ExecStart=\nExecStart={}\n", exec.render()));
        let fragment = PathBuf::from(format!("/run/user/1000/systemd/transient/{name}"));
        let unit = FakeUnit {
            transient: true,
            loaded: format!("# {}\n{text}", fragment.display()),
            text,
            fragment,
            active: "inactive".into(),
            pid: None,
        };
        self.units.borrow_mut().insert(name.clone(), unit);
        Fake::start(self, &name);
        Ok(())
    }

    fn binary_commit(&self, binary: &Path) -> Option<String> {
        self.commits.borrow().get(binary).cloned().or_else(|| {
            // Snapshot binaries report their snapshot's commit.
            binary
                .to_string_lossy()
                .contains("/snapshots/")
                .then(|| NEW.to_owned())
        })
    }

    fn file_sha256(&self, path: &Path) -> Option<String> {
        fs::read(path)
            .ok()
            .map(|bytes| format!("fake-{}", bytes.len()))
    }

    fn read_file(&self, path: &Path) -> Option<Vec<u8>> {
        if path.ends_with("agent-watch.json") {
            // The watcher completes a pass as soon as it runs.
            let running = self
                .units
                .borrow()
                .values()
                .any(|unit| unit.active == "active" && unit.loaded.contains("watch-agents"));
            let now = self
                .clock
                .get()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            return running
                .then(|| serde_json::to_vec(&json!({"checked_at": now, "agents": 3})).unwrap());
        }
        fs::read(path).ok()
    }

    fn process_exe(&self, pid: u32) -> Option<PathBuf> {
        self.exes.borrow().get(&pid).cloned()
    }

    fn process_children(&self, _pid: u32) -> Vec<u32> {
        Vec::new()
    }

    fn process_environ(&self, pid: u32) -> Option<Vec<(String, String)>> {
        self.environs.borrow().get(&pid).cloned()
    }

    fn run(
        &self,
        _program: &Path,
        args: &[String],
        _environment: &[(String, String)],
    ) -> Result<(i32, String), String> {
        assert!(args.iter().any(|arg| arg == "health"));
        let pid = self
            .units
            .borrow()
            .values()
            .find(|unit| unit.loaded.contains("chat run") && unit.active == "active")
            .and_then(|unit| unit.pid);
        if self.bridge_down.get() {
            return Ok((
                1,
                json!({"verdict": "down", "problems": {"down": ["target lead is not registered"]}})
                    .to_string(),
            ));
        }
        Ok((
            0,
            json!({
                "verdict": "ok",
                "problems": {"down": [], "degraded": []},
                "service": {"running": true, "pid": pid},
                "event_stream": {"last_item_age_seconds": 1},
            })
            .to_string(),
        ))
    }

    fn headless_status(&self, _socket: &Path) -> Result<Value, String> {
        let pid = self
            .units
            .borrow()
            .values()
            .find(|unit| unit.loaded.contains("run-headless") && unit.active == "active")
            .and_then(|unit| unit.pid);
        Ok(json!({"ok": true, "status": "idle", "busy": false, "wrapper_pid": pid}))
    }

    fn now(&self) -> SystemTime {
        self.clock.get()
    }

    fn sleep(&self, duration: Duration) {
        self.clock.set(self.clock.get() + duration);
    }
}

struct World {
    _temporary: Temporary,
    root: PathBuf,
    registry: PathBuf,
    fake: Fake,
    snapshot: Snapshot,
}

/// A directory removed when dropped.
struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        // Snapshot files are read-only; make them removable first.
        fn unlock(path: &Path) {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
                        unlock(&path);
                    }
                }
            }
        }
        unlock(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temporary(label: &str) -> Temporary {
    let path = std::env::temp_dir().join(format!(
        "agentctl-services-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    Temporary(path)
}

/// Fake release outputs and a snapshot built from them.
fn make_snapshot(host: &dyn Host, root: &Path, work: &Path) -> Snapshot {
    let release = work.join("release");
    fs::create_dir_all(&release).unwrap();
    for name in [
        "agentctl",
        "agentctl-slack-outbound",
        "agentctl-slack-plugin",
    ] {
        fs::write(release.join(name), format!("#!/bin/sh\necho {name}\n")).unwrap();
    }
    let manifest = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../chat-subscription-slack/manifest.json"),
    )
    .unwrap();
    snapshot::install_snapshot(host, root, NEW, &release, &manifest, &json!({"test": true}))
        .expect("snapshot")
}

fn transient_args(unit: &str, description: &str, props: &[&str], argv: &[&str]) -> Vec<String> {
    let mut args = vec![
        format!("--unit={unit}"),
        format!("--description={description}"),
    ];
    for prop in props {
        args.push("-p".into());
        args.push((*prop).to_owned());
    }
    args.push("--".into());
    args.extend(argv.iter().map(|word| (*word).to_owned()));
    args
}

fn world() -> World {
    let temporary = temporary("world");
    let base = temporary.0.clone();
    let root = base.join("root");
    let registry = base.join("team/.agentctl");
    fs::create_dir_all(&registry).unwrap();
    let fake = Fake::new(base.join("drop-ins"));
    let snapshot = make_snapshot(&fake, &root, &base);
    let old = "/state/bin/agentctl-old";
    fake.commits
        .borrow_mut()
        .insert(PathBuf::from(old), OLD.to_owned());
    let registry_text = registry.to_string_lossy().into_owned();
    // A bridge, as an operator makes one: a `sh -c` script with its log redirection.
    fake.systemd_run(&transient_args(
        "bridge.service",
        "chat bridge for the team",
        &[
            "Restart=on-failure",
            &format!("WorkingDirectory={}", base.join("team").display()),
            "Environment=\"AGENTCTL_HOME=/state/home-old\" \"PATH=/team/bin:/usr/bin:/bin\" \"HOME=/home/u\"",
        ],
        &[
            "/bin/sh",
            "-c",
            &format!(
                "exec {old} --registry {registry_text} chat run --bridge-state /state/bridge --offer-reply-command 2>>/state/logs/bridge.log"
            ),
        ],
    ))
    .unwrap();
    // A watcher run directly, with a relative registry and a 50% marker in its description.
    fake.systemd_run(&transient_args(
        "watch.service",
        "agent watch at 50% duty",
        &[&format!("WorkingDirectory={}", base.join("team").display())],
        &[
            old,
            "watch-agents",
            "--state-dir",
            "/state/watch",
            "--stall-minutes",
            "60",
        ],
    ))
    .unwrap();
    // Units that are not this registry's services.
    fake.systemd_run(&transient_args(
        "other-team.service",
        "another registry",
        &[],
        &[old, "--registry", "/elsewhere/.agentctl", "watch-agents"],
    ))
    .unwrap();
    fake.systemd_run(&transient_args(
        "sleeper.service",
        "not agentctl",
        &[],
        &["/bin/sleep", "100"],
    ))
    .unwrap();
    World {
        _temporary: temporary,
        root,
        registry,
        fake,
        snapshot,
    }
}

fn options(world: &World) -> RefreshOptions {
    RefreshOptions {
        snapshot: world.snapshot.clone(),
        only: Vec::new(),
        dry_run: false,
        force: false,
        health_timeout: Duration::from_secs(60),
        headless_wait: Duration::from_secs(30),
        refreshes_dir: world.root.join("refreshes"),
        drop_in_dir: world.fake.drop_in_dir.clone(),
        interrupted: None,
    }
}

#[test]
fn snapshot_is_complete_private_and_selected_as_newest() {
    let world = world();
    let snapshot = &world.snapshot;
    assert!(snapshot.path.ends_with(format!("snapshots/{}", &NEW[..12])));
    for (relative, mode) in [
        (snapshot::AGENTCTL, 0o500),
        (snapshot::OUTBOUND, 0o500),
        (snapshot::PLUGIN, 0o500),
        (snapshot::MANIFEST, 0o400),
        ("snapshot.json", 0o400),
    ] {
        let metadata = fs::metadata(snapshot.path.join(relative)).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, mode, "{relative}");
    }
    assert_eq!(snapshot.document["commit"], NEW);
    assert!(snapshot.sha256(snapshot::PLUGIN).is_some());
    assert_eq!(snapshot::select(&world.root, None).unwrap().commit, NEW);
    assert_eq!(
        snapshot::select(&world.root, Some(&NEW[..9]))
            .unwrap()
            .commit,
        NEW
    );
    assert!(snapshot::select(&world.root, Some("2222")).is_err());
    // Staging leftovers never count, and a second install of the same commit is refused.
    let leftovers: Vec<String> = fs::read_dir(snapshot::snapshots_dir(&world.root))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, [&NEW[..12]]);
}

#[test]
fn a_snapshot_whose_binary_reports_another_commit_is_not_installed() {
    let temporary = temporary("wrong-commit");
    let fake = Fake::new(temporary.0.join("drop-ins"));
    let release = temporary.0.join("release");
    fs::create_dir_all(&release).unwrap();
    for name in [
        "agentctl",
        "agentctl-slack-outbound",
        "agentctl-slack-plugin",
    ] {
        fs::write(release.join(name), "#!/bin/sh\n").unwrap();
    }
    // The fake reports NEW for snapshot binaries, so building "OLD" must fail its check.
    let error = snapshot::install_snapshot(
        &fake,
        &temporary.0.join("root"),
        OLD,
        &release,
        b"{}",
        &json!({}),
    )
    .unwrap_err();
    assert!(error.contains("reports commit"), "{error}");
    assert!(snapshot::list(&temporary.0.join("root")).is_empty());
    let entries = fs::read_dir(temporary.0.join("root/snapshots"))
        .unwrap()
        .count();
    assert_eq!(entries, 0, "the staging directory is removed");
}

#[test]
fn discovers_only_this_registrys_agentctl_services() {
    let world = world();
    let (services, unreadable) = discover(&world.fake, &Tables, &world.registry).unwrap();
    assert!(unreadable.is_empty(), "{unreadable:?}");
    let units: Vec<&str> = services
        .iter()
        .map(|service| service.unit.as_str())
        .collect();
    assert_eq!(units, ["bridge.service", "watch.service"]);
    let bridge = &services[0];
    assert_eq!(bridge.kind, Kind::Bridge);
    assert_eq!(bridge.binary, PathBuf::from("/state/bin/agentctl-old"));
    assert_eq!(bridge.bridge_state, Some(PathBuf::from("/state/bridge")));
    assert_eq!(bridge.plugin_home(), Some(PathBuf::from("/state/home-old")));
    assert_eq!(bridge.log.as_deref(), Some("2>>/state/logs/bridge.log"));
    let watcher = &services[1];
    assert_eq!(watcher.kind, Kind::Watcher);
    assert_eq!(watcher.state_dir, Some(PathBuf::from("/state/watch")));
}

#[test]
fn list_reports_binary_commit_and_snapshot_state() {
    let world = world();
    let (document, text) = super::list(&world.fake, &Tables, &world.registry, &world.root).unwrap();
    let rows = document["services"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["commit"], OLD);
    assert_eq!(rows[0]["running_binary_matches"], true);
    assert_eq!(rows[0]["newest_snapshot"]["on_snapshot"], false);
    assert!(
        text.contains("NOT on the newest snapshot (22222222)"),
        "{text}"
    );
    assert!(text.contains("plugins   /state/home-old"), "{text}");
}

#[test]
fn refresh_moves_each_service_keeping_every_argument_and_variable() {
    let world = world();
    let before_bridge = world.fake.service_text("bridge.service");
    let before_watch = world.fake.service_text("watch.service");
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let report = refresh(&world.fake, &services, &options(&world)).unwrap();
    assert_eq!(report.exit_code, 0, "{}", report.text);
    let rows = report.document["services"].as_array().unwrap();
    // The watcher goes first.
    assert_eq!(rows[0]["unit"], "watch.service");
    assert!(
        rows.iter().all(|row| row["action"] == "refreshed"),
        "{rows:?}"
    );

    let new = world.snapshot.agentctl().to_string_lossy().into_owned();
    let home = world.snapshot.home().to_string_lossy().into_owned();
    let bridge = world.fake.service_text("bridge.service");
    let expected_bridge = before_bridge
        .replace("/state/bin/agentctl-old", &new)
        .replace("/state/home-old", &home);
    assert_eq!(
        definition(&UnitText::parse(&bridge), true).unwrap(),
        definition(&UnitText::parse(&expected_bridge), true).unwrap()
    );
    let watch = world.fake.service_text("watch.service");
    assert_eq!(
        definition(&UnitText::parse(&watch), true).unwrap(),
        definition(
            &UnitText::parse(&before_watch.replace("/state/bin/agentctl-old", &new)),
            true
        )
        .unwrap()
    );
    // The description's percent sign survived the round trip, escaped as systemd writes it.
    assert!(
        watch.contains("Description=agent watch at 50%% duty"),
        "{watch}"
    );
    assert!(report
        .text
        .contains("11111111 /state/bin/agentctl-old -> 22222222"));

    // The backup and rollback commands were written, and replaying the rollback works.
    let run = PathBuf::from(report.document["run_directory"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(run.join("bridge.service.before")).unwrap(),
        before_bridge
    );
    let rollback = fs::read_to_string(run.join("ROLLBACK.sh")).unwrap();
    assert!(
        rollback.contains("systemctl --user stop bridge.service"),
        "{rollback}"
    );
    assert!(rollback.contains("/state/bin/agentctl-old"), "{rollback}");
    assert!(
        rollback.contains("AGENTCTL_HOME=/state/home-old"),
        "{rollback}"
    );
    assert!(run.join("report.json").exists());
    // A check unit ran before each real change and is gone.
    let calls = world.fake.calls();
    assert!(calls
        .iter()
        .any(|call| call.contains("--unit=agentctl-services-check-")));
    assert!(!world
        .fake
        .units
        .borrow()
        .keys()
        .any(|unit| unit.starts_with("agentctl-services-check-")));

    // A second refresh finds both current and changes nothing.
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let calls_before = world.fake.calls().len();
    let again = refresh(&world.fake, &services, &options(&world)).unwrap();
    assert_eq!(again.exit_code, 0);
    assert!(again.document["services"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["action"] == "current"));
    assert_eq!(world.fake.calls().len(), calls_before);
}

#[test]
fn dry_run_changes_and_writes_nothing() {
    let world = world();
    let before = world.fake.service_text("bridge.service");
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let mut dry = options(&world);
    dry.dry_run = true;
    let report = refresh(&world.fake, &services, &dry).unwrap();
    assert_eq!(report.exit_code, 0);
    assert!(report.text.contains("(dry run)"));
    assert!(
        report
            .text
            .contains("systemd-run --user --unit=bridge.service"),
        "{}",
        report.text
    );
    assert_eq!(world.fake.service_text("bridge.service"), before);
    assert!(world
        .fake
        .calls()
        .iter()
        .all(|call| !call.starts_with("stop")));
    assert!(!world.root.join("refreshes").exists());
}

#[test]
fn a_failed_health_check_rolls_back_and_stops_the_run() {
    let world = world();
    let before_watch = world.fake.service_text("watch.service");
    let before_bridge = world.fake.service_text("bridge.service");
    *world.fake.crashing.borrow_mut() = Some(world.snapshot.agentctl());
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let report = refresh(&world.fake, &services, &options(&world)).unwrap();
    assert_eq!(report.exit_code, 1, "{}", report.text);
    let rows = report.document["services"].as_array().unwrap();
    assert_eq!(rows[0]["unit"], "watch.service");
    assert_eq!(rows[0]["action"], "rolled-back", "{rows:?}");
    assert_eq!(rows[1]["action"], "not-attempted");
    // The watcher runs its old binary again with its old definition; the bridge was never touched.
    assert_eq!(
        definition(
            &UnitText::parse(&world.fake.service_text("watch.service")),
            true
        )
        .unwrap(),
        definition(&UnitText::parse(&before_watch), true).unwrap()
    );
    let pid = world.fake.pid("watch.service").unwrap();
    assert_eq!(
        world.fake.process_exe(pid),
        Some(PathBuf::from("/state/bin/agentctl-old"))
    );
    assert_eq!(world.fake.service_text("bridge.service"), before_bridge);
    assert!(report.text.contains("rolled back to"), "{}", report.text);
}

#[test]
fn a_property_systemd_run_refuses_stops_before_the_unit_is_touched() {
    let world = world();
    let before = world.fake.service_text("watch.service");
    let pid = world.fake.pid("watch.service");
    *world.fake.refuse_property.borrow_mut() = Some("WorkingDirectory".into());
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let mut only = options(&world);
    only.only = vec!["watch".into()];
    let report = refresh(&world.fake, &services, &only).unwrap();
    assert_eq!(report.exit_code, 1);
    assert_eq!(report.document["services"][0]["action"], "failed-unchanged");
    assert_eq!(world.fake.service_text("watch.service"), before);
    assert_eq!(world.fake.pid("watch.service"), pid);
    assert!(!world
        .fake
        .calls()
        .iter()
        .any(|call| call == "stop watch.service"));
}

#[test]
fn only_names_an_existing_service() {
    let world = world();
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let mut only = options(&world);
    only.only = vec!["sleeper".into()];
    assert!(refresh(&world.fake, &services, &only).is_err());
}

#[test]
fn a_unit_file_service_gets_a_drop_in_and_loses_it_on_rollback() {
    let world = world();
    let registry = world.registry.to_string_lossy().into_owned();
    world.fake.add_unit_file(
        "headless.service",
        &format!(
            "[Unit]\nDescription=headless wrapper\n\n[Service]\nEnvironment=PATH=/usr/bin:/bin\nExecStart=/state/bin/agentctl-old --registry {registry} run-headless --approval-timeout 600 lead\nRestart=on-failure\n"
        ),
    );
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let headless = services
        .iter()
        .find(|service| service.unit == "headless.service")
        .unwrap();
    assert_eq!(headless.kind, Kind::Headless);
    assert_eq!(headless.agent.as_deref(), Some("lead"));

    let mut only = options(&world);
    only.only = vec!["headless.service".into()];
    let report = refresh(&world.fake, &services, &only).unwrap();
    assert_eq!(report.exit_code, 0, "{}", report.text);
    let drop_in = world
        .fake
        .drop_in_dir
        .join("headless.service.d")
        .join(DROP_IN);
    let content = fs::read_to_string(&drop_in).unwrap();
    assert!(content.contains("ExecStart=\n"), "{content}");
    assert!(content.contains(&world.snapshot.agentctl().to_string_lossy().into_owned()));
    let pid = world.fake.pid("headless.service").unwrap();
    assert_eq!(world.fake.process_exe(pid), Some(world.snapshot.agentctl()));
    let calls = world.fake.calls();
    assert!(calls.contains(&"daemon-reload".to_owned()));
    assert!(calls.contains(&"restart headless.service".to_owned()));

    // Now a refresh that fails its check: the drop-in is removed and the old binary runs again.
    fs::remove_file(&drop_in).unwrap();
    world.fake.daemon_reload().unwrap();
    world.fake.restart("headless.service").unwrap();
    *world.fake.crashing.borrow_mut() = Some(world.snapshot.agentctl());
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let report = refresh(&world.fake, &services, &only).unwrap();
    assert_eq!(report.exit_code, 1, "{}", report.text);
    assert_eq!(report.document["services"][0]["action"], "rolled-back");
    assert!(!drop_in.exists());
    let pid = world.fake.pid("headless.service").unwrap();
    assert_eq!(
        world.fake.process_exe(pid),
        Some(PathBuf::from("/state/bin/agentctl-old"))
    );
}

#[test]
fn definitions_compare_words_not_quoting() {
    let left = UnitText::parse("[Service]\nExecStart=/bin/a \"b c\"\nEnvironment=X=1 Y=2\n");
    let right = UnitText::parse(
        "[Service]\nExecStart=\"/bin/a\" 'b c'\nEnvironment=\"X=1\"\nEnvironment=Y=2\n",
    );
    assert_eq!(
        definition(&left, false).unwrap(),
        definition(&right, false).unwrap()
    );
}

#[test]
fn a_unit_that_stays_loaded_after_stop_is_started_again_from_its_own_definition() {
    let world = world();
    let before = world.fake.service_text("watch.service");
    world.fake.sticky.set(true);
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let mut only = options(&world);
    only.only = vec!["watch.service".into()];
    let report = refresh(&world.fake, &services, &only).unwrap();
    assert_eq!(report.exit_code, 1, "{}", report.text);
    let row = &report.document["services"][0];
    assert_eq!(row["action"], "rolled-back", "{row}");
    assert!(
        row["error"].as_str().unwrap().contains("stays loaded"),
        "{row}"
    );
    assert_eq!(world.fake.service_text("watch.service"), before);
    let pid = world.fake.pid("watch.service").unwrap();
    assert_eq!(
        world.fake.process_exe(pid),
        Some(PathBuf::from("/state/bin/agentctl-old"))
    );
    assert!(world
        .fake
        .calls()
        .contains(&"start watch.service".to_owned()));
}

#[test]
fn a_service_unhealthy_before_the_refresh_is_left_alone() {
    let world = world();
    let before = world.fake.service_text("bridge.service");
    world.fake.bridge_down.set(true);
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let mut only = options(&world);
    only.only = vec!["bridge".into()];
    let report = refresh(&world.fake, &services, &only).unwrap();
    assert_eq!(report.exit_code, 1, "{}", report.text);
    assert_eq!(report.document["services"][0]["action"], "failed-unchanged");
    assert!(
        report.text.contains("not healthy before the refresh"),
        "{}",
        report.text
    );
    assert_eq!(world.fake.service_text("bridge.service"), before);
    assert!(!world.fake.calls().iter().any(|call| call.contains("check")));
}

#[test]
fn an_interrupted_refresh_attempts_nothing_more() {
    let world = world();
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let mut interrupted = options(&world);
    interrupted.interrupted = Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        true,
    )));
    let report = refresh(&world.fake, &services, &interrupted).unwrap();
    assert_eq!(report.exit_code, 1);
    assert!(report.document["services"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["action"] == "not-attempted"));
    assert!(world
        .fake
        .calls()
        .iter()
        .all(|call| !call.starts_with("stop")));
}

#[test]
fn a_transient_unit_with_a_drop_in_is_refused() {
    let world = world();
    let (mut services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let watcher = services
        .iter_mut()
        .find(|service| service.unit == "watch.service")
        .unwrap();
    watcher.text.push_str("\n# /home/u/.config/systemd/user.control/watch.service.d/50-CPUQuota.conf\n[Service]\nCPUQuota=50%\n");
    watcher.parsed = UnitText::parse(&watcher.text);
    let error =
        super::refresh::plan(watcher, &world.snapshot, &world.fake.drop_in_dir, false).unwrap_err();
    assert!(error.contains("drop-ins"), "{error}");
}

#[test]
fn the_bridge_must_run_with_the_snapshots_plugin_home() {
    let world = world();
    let (services, _) = discover(&world.fake, &Tables, &world.registry).unwrap();
    let bridge = services
        .iter()
        .find(|service| service.unit == "bridge.service")
        .unwrap();
    let pid = world.fake.pid("bridge.service").unwrap();
    // The old process runs with the old home; a check that expects the snapshot's must fail.
    let expected = vec![(
        "AGENTCTL_HOME".to_owned(),
        world.snapshot.home().to_string_lossy().into_owned(),
    )];
    let error = super::refresh::health(
        &world.fake,
        bridge,
        Path::new("/state/bin/agentctl-old"),
        &expected,
        UNIX_EPOCH,
        Duration::from_secs(60),
    )
    .unwrap_err();
    assert!(
        error.contains(&format!(
            "process {pid} runs with AGENTCTL_HOME /state/home-old"
        )),
        "{error}"
    );
}
