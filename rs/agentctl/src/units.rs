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
    if !options.dest.is_absolute() {
        return Err(fail("--dest must be an absolute directory"));
    }
    let files = rendered(options)?;
    let mut written = Vec::new();
    let mut unchanged = Vec::new();
    let mut conflicts = Vec::new();
    for (relative, contents) in &files {
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
        for (relative, contents) in &files {
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
