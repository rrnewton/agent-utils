//! `agentctl install-units`: write the user units that restore a coordinator team at boot.
//!
//! The templates live beside this file in `systemd/`. Installing renders their absolute paths
//! and writes them; it never enables, starts, or reloads anything, so an operator reviews the
//! files and enables them deliberately.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::agent::AgentError;

const HERDR_SERVER: &str = include_str!("systemd/herdr-server.service");
const AGENT_RESTORE: &str = include_str!("systemd/agent-restore.service");
const AGENT_WATCH: &str = include_str!("systemd/agent-watch.service");
const BRIDGE_AFTER_HERDR: &str = include_str!("systemd/bridge-after-herdr.conf");
const HEADLESS: &str = include_str!("systemd/agentctl-headless.service");
const BRIDGE: &str = include_str!("systemd/agentctl-bridge.service");
const SCHEDULED_SERVICE: &str = include_str!("systemd/agentctl-scheduled.service");
const SCHEDULED_TIMER: &str = include_str!("systemd/agentctl-scheduled.timer");

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// What to render and where.
#[derive(Clone, Debug)]
pub struct InstallOptions {
    /// Destination directory, normally `~/.config/systemd/user`.
    pub dest: PathBuf,
    /// The agentctl executable the units run.
    pub agentctl: PathBuf,
    /// The Herdr executable.
    pub herdr: PathBuf,
    /// The agent registry restore and watch operate on.
    pub registry: PathBuf,
    /// The chat bridge unit to order after the Herdr server, if any.
    pub bridge_unit: Option<String>,
    /// Report what would be written without writing.
    pub dry_run: bool,
    /// Replace existing files that differ.
    pub force: bool,
}

/// A path systemd takes literally in an `ExecStart=` line: absolute, with no white space,
/// quote, backslash, control character, or `$`; `%` is escaped as `%%`.
fn unit_path(label: &str, path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| fail(format!("{label} path is not UTF-8")))?;
    if !path.is_absolute()
        || text.chars().any(|character| {
            character.is_whitespace() || character.is_control() || "\"'\\$;".contains(character)
        })
    {
        return Err(fail(format!(
            "{label} must be an absolute path without white space, quotes, backslashes, `$`, or `;`: {text}"
        )));
    }
    Ok(text.replace('%', "%%"))
}

/// The files `install` writes, rendered, as `(relative path, contents)`.
pub fn rendered(options: &InstallOptions) -> Result<Vec<(PathBuf, String)>> {
    let agentctl = unit_path("agentctl", &options.agentctl)?;
    let herdr = unit_path("herdr", &options.herdr)?;
    let registry = unit_path("registry", &options.registry)?;
    let render = |template: &str| {
        template
            .replace("@AGENTCTL@", &agentctl)
            .replace("@HERDR@", &herdr)
            .replace("@REGISTRY@", &registry)
    };
    let mut files = vec![
        (PathBuf::from("herdr-server.service"), render(HERDR_SERVER)),
        (
            PathBuf::from("agent-restore.service"),
            render(AGENT_RESTORE),
        ),
        (PathBuf::from("agent-watch.service"), render(AGENT_WATCH)),
    ];
    if let Some(unit) = options.bridge_unit.as_deref() {
        if !crate::restore::unit_name(unit) || !unit.ends_with(".service") {
            return Err(fail(format!(
                "--bridge-unit must be a .service unit name: {unit}"
            )));
        }
        // Drop-ins apply in file-name order, so the name sorts after the usual numbered ones; the
        // drop-in sets no start limit, which stays with the bridge's own policy.
        files.push((
            PathBuf::from(format!("{unit}.d")).join("zz-agentctl-after-herdr.conf"),
            BRIDGE_AFTER_HERDR.to_owned(),
        ));
    }
    Ok(files)
}

/// Write the rendered units; refuse to replace a different existing file without `force`.
pub fn install(options: &InstallOptions) -> Result<Value> {
    let files = rendered(options)?;
    let (written, unchanged) = write_files(options, &files)?;
    let enable = [
        "herdr-server.service",
        "agent-restore.service",
        "agent-watch.service",
    ];
    Ok(json!({
        "dry_run": options.dry_run,
        "written": written,
        "unchanged": unchanged,
        "next_steps": [
            "review the files",
            "systemctl --user daemon-reload",
            format!("systemctl --user enable {}", enable.join(" ")),
            "loginctl enable-linger \"$USER\"   # once, so the units start at boot without a login",
        ],
    }))
}

/// What the headless unit takes from the installing shell.
#[derive(Clone, Debug, Default)]
pub struct HeadlessUnit {
    /// The Claude Code executable, passed as `run-headless --claude-bin`.
    pub claude: Option<PathBuf>,
    /// `(name, value)` pairs written as `Environment=` lines, such as PATH.
    pub environment: Vec<(String, String)>,
}

/// One `Environment=` line, quoted for systemd, or `None` for a value it cannot carry.
fn environment_line(name: &str, value: &str) -> Option<String> {
    let valid_name = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if !valid_name
        || value
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        return None;
    }
    Some(format!(
        "Environment=\"{name}={}\"\n",
        value.replace('%', "%%").replace('$', "$$")
    ))
}

/// The unit that runs the headless agent `name`, rendered, as `(file name, contents)`.
pub fn rendered_headless(
    options: &InstallOptions,
    name: &str,
    unit: &HeadlessUnit,
) -> Result<(PathBuf, String)> {
    if name.is_empty()
        || name.len() > 32
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(fail(format!(
            "--headless takes an agent name (1-32 lowercase letters, digits, and hyphens), not {name:?}"
        )));
    }
    let agentctl = unit_path("agentctl", &options.agentctl)?;
    let registry = unit_path("registry", &options.registry)?;
    let claude = match unit.claude.as_deref() {
        Some(path) => format!(" --claude-bin {}", unit_path("claude", path)?),
        None => String::new(),
    };
    let environment: String = unit
        .environment
        .iter()
        .filter_map(|(key, value)| environment_line(key, value))
        .collect();
    Ok((
        PathBuf::from(format!("agentctl-headless-{name}.service")),
        HEADLESS
            .replace("@AGENTCTL@", &agentctl)
            .replace("@REGISTRY@", &registry)
            .replace("@ENVIRONMENT@", &environment)
            .replace("@CLAUDE_BIN@", &claude)
            .replace("@NAME@", name),
    ))
}

/// A bridge name and unit name take the same shape as an agent name.
fn bridge_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 32
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(fail(format!(
            "--bridge takes a bridge name (1-32 lowercase letters, digits, and hyphens), not {name:?}"
        )));
    }
    Ok(())
}

/// The persistent unit that runs the chat bridge `name`, rendered, as `(file name,
/// contents)`. Its state directory is `REGISTRY/bridges/NAME`, where `chat run` keeps it.
pub fn rendered_bridge(options: &InstallOptions, name: &str) -> Result<(PathBuf, String)> {
    bridge_name(name)?;
    let agentctl = unit_path("agentctl", &options.agentctl)?;
    let registry = unit_path("registry", &options.registry)?;
    let state = options.registry.join("bridges").join(name);
    let state = unit_path("bridge state", &state)?;
    Ok((
        PathBuf::from(format!("agentctl-bridge-{name}.service")),
        BRIDGE
            .replace("@AGENTCTL@", &agentctl)
            .replace("@REGISTRY@", &registry)
            .replace("@BRIDGE_STATE@", &state)
            .replace("@NAME@", name),
    ))
}

/// Write only the unit of the chat bridge `name`; refuse to replace a different existing
/// file without `force`. Enables and starts nothing.
pub fn install_bridge(options: &InstallOptions, name: &str) -> Result<Value> {
    let file = rendered_bridge(options, name)?;
    let unit = file.0.display().to_string();
    let (written, unchanged) = write_files(options, &[file])?;
    Ok(json!({
        "dry_run": options.dry_run,
        "written": written,
        "unchanged": unchanged,
        "next_steps": [
            "review the file",
            "systemctl --user daemon-reload",
            format!("systemctl --user enable --now {unit}"),
        ],
    }))
}

/// One argument of a scheduled command as systemd takes it literally in an
/// `ExecStart=` line: no white space, quotes, backslash, control character, `$`, `;`,
/// or backtick; `%` is escaped as `%%`.
fn scheduled_argument(argument: &str) -> Result<String> {
    if argument.is_empty()
        || argument.chars().any(|character| {
            character.is_whitespace() || character.is_control() || "\"'\\$;`".contains(character)
        })
    {
        return Err(fail(format!(
            "scheduled command argument cannot be rendered literally: {argument:?}"
        )));
    }
    Ok(argument.replace('%', "%%"))
}

/// The `ConditionHost=` value: this host's name, taken literally.
fn scheduled_host(host: &str) -> Result<String> {
    if host.is_empty()
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(fail(format!(
            "host name cannot be rendered in ConditionHost: {host:?}"
        )));
    }
    Ok(host.to_owned())
}

/// The persistent timer/service pair one `scheduled` spec entry renders as, on the
/// host named `host`: `(relative path, contents)` for the service, then the timer.
pub fn rendered_scheduled(
    entry: &crate::restore::ScheduledUnit,
    host: &str,
) -> Result<Vec<(PathBuf, String)>> {
    bridge_name(&entry.name)?;
    let host = scheduled_host(host)?;
    if !crate::restore::calendar_schedule(&entry.on_calendar) {
        return Err(fail(format!(
            "scheduled {:?} on_calendar is not a plain calendar schedule: {:?}",
            entry.name, entry.on_calendar
        )));
    }
    let Some(program) = entry.command.first() else {
        return Err(fail(format!(
            "scheduled {:?} command must not be empty",
            entry.name
        )));
    };
    let mut rendered = unit_path("scheduled program", Path::new(program))?;
    for argument in &entry.command[1..] {
        rendered.push(' ');
        rendered.push_str(&scheduled_argument(argument)?);
    }
    let base = format!("agentctl-scheduled-{}", entry.name);
    Ok(vec![
        (
            PathBuf::from(format!("{base}.service")),
            SCHEDULED_SERVICE
                .replace("@HOST@", &host)
                .replace("@COMMAND@", &rendered)
                .replace("@NAME@", &entry.name),
        ),
        (
            PathBuf::from(format!("{base}.timer")),
            SCHEDULED_TIMER
                .replace("@HOST@", &host)
                .replace("@SCHEDULE@", &entry.on_calendar.replace('%', "%%"))
                .replace("@NAME@", &entry.name),
        ),
    ])
}

/// Write every `scheduled` entry of the registry's team spec as a persistent
/// timer/service pair for this host; refuse to replace a different existing file
/// without `force`. Enables and starts nothing.
pub fn install_scheduled(options: &InstallOptions) -> Result<Value> {
    let config = crate::restore::RestoreConfig::load(&options.registry)?;
    if config.scheduled.is_empty() {
        return Err(fail(
            "the team spec (restore.json) names no scheduled entries; add a \
             \"scheduled\" list of {name, on_calendar, command} first",
        ));
    }
    let host = crate::subagents::local_hostname()
        .ok_or_else(|| fail("cannot determine this host's name for the units' ConditionHost"))?;
    let mut files = Vec::new();
    let mut timers = Vec::new();
    for entry in &config.scheduled {
        files.extend(rendered_scheduled(entry, &host)?);
        timers.push(format!("agentctl-scheduled-{}.timer", entry.name));
    }
    let (written, unchanged) = write_files(options, &files)?;
    Ok(json!({
        "dry_run": options.dry_run,
        "written": written,
        "unchanged": unchanged,
        "next_steps": [
            "review the files",
            "systemctl --user daemon-reload",
            format!("systemctl --user enable --now {}", timers.join(" ")),
        ],
    }))
}

/// Write only the unit of the headless agent `name`; refuse to replace a different existing
/// file without `force`. Enables and starts nothing.
pub fn install_headless(
    options: &InstallOptions,
    name: &str,
    unit: &HeadlessUnit,
) -> Result<Value> {
    let file = rendered_headless(options, name, unit)?;
    let unit = file.0.display().to_string();
    let (written, unchanged) = write_files(options, &[file])?;
    Ok(json!({
        "dry_run": options.dry_run,
        "written": written,
        "unchanged": unchanged,
        "next_steps": [
            "review the file",
            "systemctl --user daemon-reload",
            format!("systemctl --user enable --now {unit}"),
            format!("agentctl --registry {} tail {name} --follow", options.registry.display()),
        ],
    }))
}

/// Write `files` under `options.dest`, returning the paths written and those already current.
fn write_files(
    options: &InstallOptions,
    files: &[(PathBuf, String)],
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    if !options.dest.is_absolute() {
        return Err(fail("--dest must be an absolute directory"));
    }
    let mut written = Vec::new();
    let mut unchanged = Vec::new();
    let mut conflicts = Vec::new();
    for (relative, contents) in files {
        let path = options.dest.join(relative);
        match fs::read_to_string(&path) {
            Ok(existing) if existing == *contents => unchanged.push(path),
            Ok(_) if !options.force => conflicts.push(path),
            Ok(_) | Err(_) => written.push(path),
        }
    }
    if !conflicts.is_empty() {
        return Err(fail(format!(
            "these files exist with different contents; inspect them, then rerun with --force: {}",
            conflicts
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if !options.dry_run {
        for (relative, contents) in files {
            let path = options.dest.join(relative);
            if !written.contains(&path) {
                continue;
            }
            let directory = path.parent().expect("unit file has a directory");
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o755)
                .create(directory)
                .map_err(|error| fail(format!("cannot create {}: {error}", directory.display())))?;
            let temporary = directory.join(format!(
                ".{}.agentctl-install",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unit")
            ));
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o644)
                .open(&temporary)
                .map_err(|error| fail(format!("cannot write {}: {error}", temporary.display())))?;
            file.write_all(contents.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|error| fail(format!("cannot write {}: {error}", temporary.display())))?;
            fs::rename(&temporary, &path)
                .map_err(|error| fail(format!("cannot install {}: {error}", path.display())))?;
        }
    }
    Ok((written, unchanged))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(dest: &Path) -> InstallOptions {
        InstallOptions {
            dest: dest.to_owned(),
            agentctl: PathBuf::from("/opt/agentctl/bin/agentctl"),
            herdr: PathBuf::from("/home/owner/bin/herdr"),
            registry: PathBuf::from("/home/owner/project/.agentctl"),
            bridge_unit: Some("chat-bridge.service".to_owned()),
            dry_run: false,
            force: false,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("agentctl-units-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn headless_unit_runs_the_wrapper_and_is_written_alone() {
        let dest = scratch("headless");
        let unit = HeadlessUnit {
            claude: Some(PathBuf::from("/usr/local/bin/claude")),
            environment: vec![
                ("PATH".to_owned(), "/home/owner/bin:/usr/bin".to_owned()),
                ("BAD".to_owned(), "a\"b".to_owned()),
                ("PCT".to_owned(), "50%$x".to_owned()),
            ],
        };
        let (name, contents) = rendered_headless(&options(&dest), "coordinator", &unit).unwrap();
        assert_eq!(name, PathBuf::from("agentctl-headless-coordinator.service"));
        assert!(contents.contains(
            "ExecStart=/opt/agentctl/bin/agentctl --registry /home/owner/project/.agentctl run-headless coordinator --claude-bin /usr/local/bin/claude\n"
        ), "{contents}");
        assert!(
            contents.contains("Environment=\"PATH=/home/owner/bin:/usr/bin\"\n"),
            "{contents}"
        );
        assert!(
            contents.contains("Environment=\"PCT=50%%$$x\"\n"),
            "{contents}"
        );
        assert!(!contents.contains("BAD"), "{contents}");
        assert!(contents.contains("Restart=on-failure"));
        assert!(!contents.contains('@'), "{contents}");
        let bare = rendered_headless(&options(&dest), "coordinator", &HeadlessUnit::default())
            .unwrap()
            .1;
        assert!(bare.contains("run-headless coordinator\n"), "{bare}");
        assert!(!bare.contains("Environment="), "{bare}");
        assert!(rendered_headless(&options(&dest), "../x", &unit).is_err());
        let report = install_headless(&options(&dest), "coordinator", &unit).unwrap();
        assert_eq!(report["written"].as_array().unwrap().len(), 1);
        assert!(!dest.join("agent-watch.service").exists());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn bridge_unit_is_persistent_and_written_alone() {
        let dest = scratch("bridge");
        let (name, contents) = rendered_bridge(&options(&dest), "coordinator").unwrap();
        assert_eq!(name, PathBuf::from("agentctl-bridge-coordinator.service"));
        assert!(
            contents.contains(
                "ExecStart=/opt/agentctl/bin/agentctl --registry /home/owner/project/.agentctl \
             chat run --bridge-state /home/owner/project/.agentctl/bridges/coordinator\n"
            ),
            "{contents}"
        );
        assert!(
            contents.contains("After=herdr-server.service"),
            "{contents}"
        );
        assert!(contents.contains("Restart=on-failure"), "{contents}");
        assert!(contents.contains("WantedBy=default.target"), "{contents}");
        assert!(!contents.contains('@'), "{contents}");
        assert!(rendered_bridge(&options(&dest), "../x").is_err());
        let report = install_bridge(&options(&dest), "coordinator").unwrap();
        assert_eq!(report["written"].as_array().unwrap().len(), 1);
        assert!(dest.join("agentctl-bridge-coordinator.service").is_file());
        assert!(!dest.join("agent-watch.service").exists());
        let _ = fs::remove_dir_all(&dest);
    }

    fn scheduled(name: &str, on_calendar: &str, command: &[&str]) -> crate::restore::ScheduledUnit {
        crate::restore::ScheduledUnit {
            name: name.to_owned(),
            on_calendar: on_calendar.to_owned(),
            command: command.iter().map(|item| item.to_string()).collect(),
        }
    }

    #[test]
    fn scheduled_pair_is_persistent_host_bound_and_fully_rendered() {
        let entry = scheduled(
            "nightly-backup",
            "Mon 06:00",
            &["/home/owner/bin/backup", "--quiet", "50%"],
        );
        let files = rendered_scheduled(&entry, "host-one.example").unwrap();
        let names = files
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "agentctl-scheduled-nightly-backup.service",
                "agentctl-scheduled-nightly-backup.timer"
            ]
        );
        let (service, timer) = (&files[0].1, &files[1].1);
        assert!(
            service.contains("ConditionHost=host-one.example\n"),
            "{service}"
        );
        assert!(
            service.contains("ExecStart=/home/owner/bin/backup --quiet 50%%\n"),
            "{service}"
        );
        assert!(service.contains("Type=oneshot"), "{service}");
        assert!(
            timer.contains("ConditionHost=host-one.example\n"),
            "{timer}"
        );
        assert!(timer.contains("OnCalendar=Mon 06:00\n"), "{timer}");
        assert!(
            timer.contains("Unit=agentctl-scheduled-nightly-backup.service\n"),
            "{timer}"
        );
        assert!(timer.contains("Persistent=true\n"), "{timer}");
        assert!(timer.contains("WantedBy=timers.target\n"), "{timer}");
        for (_, contents) in &files {
            assert!(!contents.contains('@'), "{contents}");
        }
    }

    #[test]
    fn scheduled_rendering_refuses_unrenderable_entries() {
        for entry in [
            scheduled("Bad Name", "daily", &["/bin/true"]),
            scheduled("ok", "daily; rm -rf /", &["/bin/true"]),
            scheduled("ok", "daily", &["true"]),
            scheduled("ok", "daily", &[]),
            scheduled("ok", "daily", &["/bin/echo", "two words"]),
            scheduled("ok", "daily", &["/bin/echo", "$HOME"]),
        ] {
            assert!(rendered_scheduled(&entry, "host-one").is_err(), "{entry:?}");
        }
        assert!(rendered_scheduled(&scheduled("ok", "daily", &["/bin/true"]), "bad host").is_err());
    }

    #[test]
    fn install_scheduled_writes_every_spec_pair_and_nothing_else() {
        let dest = scratch("scheduled");
        let registry = dest.join("registry");
        fs::create_dir_all(&registry).unwrap();
        let mut chosen = options(&dest);
        chosen.registry = registry.clone();
        // Without a spec entry there is nothing to write, and that is said out loud.
        assert!(install_scheduled(&chosen).is_err());
        fs::write(
            registry.join("restore.json"),
            r#"{"schema": "agentctl-restore/v1", "scheduled": [
                {"name": "nightly-backup", "on_calendar": "daily", "command": ["/home/owner/bin/backup"]},
                {"name": "hourly-tick", "on_calendar": "hourly", "command": ["/home/owner/bin/tick"]}
            ]}"#,
        )
        .unwrap();
        let host = crate::subagents::local_hostname().unwrap();
        let report = install_scheduled(&chosen).unwrap();
        assert_eq!(report["written"].as_array().unwrap().len(), 4);
        for name in [
            "agentctl-scheduled-nightly-backup.service",
            "agentctl-scheduled-nightly-backup.timer",
            "agentctl-scheduled-hourly-tick.service",
            "agentctl-scheduled-hourly-tick.timer",
        ] {
            assert!(dest.join(name).is_file(), "{name}");
        }
        assert!(!dest.join("agent-watch.service").exists());
        let timer = fs::read_to_string(dest.join("agentctl-scheduled-hourly-tick.timer")).unwrap();
        assert!(
            timer.contains(&format!("ConditionHost={host}\n")),
            "{timer}"
        );
        assert!(
            report["next_steps"][2]
                .as_str()
                .unwrap()
                .contains("enable --now agentctl-scheduled-nightly-backup.timer"),
            "{report}"
        );
        let again = install_scheduled(&chosen).unwrap();
        assert_eq!(again["unchanged"].as_array().unwrap().len(), 4);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn rendering_fills_every_placeholder_with_absolute_paths() {
        let files = rendered(&options(Path::new("/unused"))).unwrap();
        let names = files
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "herdr-server.service",
                "agent-restore.service",
                "agent-watch.service",
                "chat-bridge.service.d/zz-agentctl-after-herdr.conf"
            ]
        );
        for (_, contents) in &files {
            assert!(!contents.contains('@'), "{contents}");
        }
        assert!(files[0]
            .1
            .contains("ExecStart=/bin/bash -lc 'exec \"$$0\" server' /home/owner/bin/herdr"));
        assert!(files[0].1.contains("KillMode=process"));
        assert!(files[1].1.contains(
            "ExecStart=/opt/agentctl/bin/agentctl --registry /home/owner/project/.agentctl --herdr-bin /home/owner/bin/herdr restore --boot"
        ));
        assert!(files[1].1.contains("After=herdr-server.service"));
        assert!(files[2].1.contains("watch\n"));
        assert!(!files[2].1.contains("After=herdr-server.service"));
        assert!(files[3].1.contains("After=herdr-server.service"));
        assert!(files[3].1.contains("Restart=on-failure"));
        // A crash-looping bridge must not get a higher start burst than its own policy allows.
        assert!(!files[3]
            .1
            .lines()
            .any(|line| line.starts_with("StartLimit")));
    }

    #[test]
    fn unsafe_paths_and_unit_names_are_refused() {
        for (agentctl, bridge) in [
            ("/opt/agent ctl", None),
            ("relative/agentctl", None),
            ("/opt/$HOME/agentctl", None),
            ("/opt/agentctl", Some("--now")),
            ("/opt/agentctl", Some("bridge.timer")),
        ] {
            let mut chosen = options(Path::new("/unused"));
            chosen.agentctl = PathBuf::from(agentctl);
            chosen.bridge_unit = bridge.map(str::to_owned);
            assert!(rendered(&chosen).is_err(), "{agentctl} {bridge:?}");
        }
        let mut percent = options(Path::new("/unused"));
        percent.registry = PathBuf::from("/home/owner/100%/.agentctl");
        assert!(rendered(&percent).unwrap()[1]
            .1
            .contains("/home/owner/100%%/.agentctl"));
    }

    #[test]
    fn install_writes_once_refuses_a_different_file_and_never_enables() {
        let dest = scratch("install");
        let dry = install(&InstallOptions {
            dry_run: true,
            ..options(&dest)
        })
        .unwrap();
        assert_eq!(dry["written"].as_array().unwrap().len(), 4);
        assert!(!dest.exists());
        let first = install(&options(&dest)).unwrap();
        assert_eq!(first["written"].as_array().unwrap().len(), 4);
        assert!(dest
            .join("chat-bridge.service.d/zz-agentctl-after-herdr.conf")
            .is_file());
        let again = install(&options(&dest)).unwrap();
        assert_eq!(again["unchanged"].as_array().unwrap().len(), 4);
        fs::write(dest.join("agent-watch.service"), "# edited by hand\n").unwrap();
        assert!(install(&options(&dest)).is_err());
        assert_eq!(
            fs::read_to_string(dest.join("agent-watch.service")).unwrap(),
            "# edited by hand\n"
        );
        let forced = install(&InstallOptions {
            force: true,
            ..options(&dest)
        })
        .unwrap();
        assert_eq!(forced["written"].as_array().unwrap().len(), 1);
        assert!(forced["next_steps"][2]
            .as_str()
            .unwrap()
            .starts_with("systemctl --user enable"));
        fs::remove_dir_all(&dest).unwrap();
    }
}
