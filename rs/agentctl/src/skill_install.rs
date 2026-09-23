//! Idempotent installation of the bundled agentctl skill.
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::agent::AgentError;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

#[derive(Debug, Serialize)]
pub(crate) struct InstallResult {
    installed: Vec<String>,
    unchanged: Vec<String>,
}

fn direct_root(harness: &str) -> Result<PathBuf, AgentError> {
    if let Some(value) = std::env::var_os(format!(
        "AGENTCTL_{}_SKILLS_DIR",
        harness.to_ascii_uppercase()
    )) {
        let root = PathBuf::from(value);
        if !root.is_absolute() {
            return Err(fail(format!(
                "{harness} skill directory override must be absolute"
            )));
        }
        return Ok(root);
    }
    let directory = match harness {
        "codex" => ".codex",
        "claude" => ".claude",
        _ => {
            return Err(fail(format!(
                "unsupported direct skill harness {harness:?}"
            )))
        }
    };
    Ok(crate::client::account_home()?
        .join(directory)
        .join("skills"))
}

fn ensure_real_directory(path: &Path) -> Result<(), AgentError> {
    if !path.is_absolute() {
        return Err(fail(format!(
            "skill destination must be absolute: {}",
            path.display()
        )));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(fail(format!(
                    "refusing non-directory skill destination: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                DirBuilder::new()
                    .mode(0o700)
                    .create(&current)
                    .map_err(|error| {
                        fail(format!(
                            "cannot create skill destination {}: {error}",
                            current.display()
                        ))
                    })?;
            }
            Err(error) => {
                return Err(fail(format!(
                    "cannot inspect skill destination {}: {error}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

fn existing(path: &Path, force: bool) -> Result<Option<&'static str>, AgentError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(fail(format!(
                "cannot inspect installed skill {}: {error}",
                path.display()
            )));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(fail(format!(
            "refusing non-regular skill file: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| {
        fail(format!(
            "cannot inspect installed skill {}: {error}",
            path.display()
        ))
    })?;
    if bytes == crate::AGENTCTL_SKILL.as_bytes() {
        return Ok(Some("unchanged"));
    }
    if !force {
        return Err(fail(format!(
            "refusing to overwrite divergent skill {}; inspect it or pass --force",
            path.display()
        )));
    }
    Ok(None)
}

fn write_direct(path: &Path, force: bool) -> Result<&'static str, AgentError> {
    let directory = path.parent().expect("skill parent");
    ensure_real_directory(directory)?;
    if let Some(outcome) = existing(path, force)? {
        return Ok(outcome);
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| fail(error.to_string()))?
        .as_nanos();
    let temporary = directory.join(format!(".SKILL.md.{}.{nonce}", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| fail(error.to_string()))?;
    let result = (|| {
        file.write_all(crate::AGENTCTL_SKILL.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(directory)?.sync_all()?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(fail(error.to_string()));
    }
    Ok("installed")
}

fn muse_config_home() -> Result<PathBuf, AgentError> {
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME") {
        let root = PathBuf::from(value);
        if !root.is_absolute() {
            return Err(fail(
                "XDG_CONFIG_HOME must be absolute for Muse skill installation",
            ));
        }
        return Ok(root.join("muse"));
    }
    Ok(crate::client::account_home()?.join(".config/muse"))
}

fn muse_executable() -> Result<PathBuf, AgentError> {
    let Some(configured) = std::env::var_os("AGENTCTL_MUSE_BIN") else {
        return Ok(crate::client::resolve_harness_executable("muse")?);
    };
    let path = PathBuf::from(configured);
    if !path.is_absolute() {
        return Err(fail("AGENTCTL_MUSE_BIN must be an absolute path"));
    }
    let resolved = fs::canonicalize(&path).map_err(|error| {
        fail(format!(
            "cannot inspect Muse executable {}: {error}",
            path.display()
        ))
    })?;
    let metadata = fs::metadata(&resolved).map_err(|error| {
        fail(format!(
            "cannot inspect Muse executable {}: {error}",
            resolved.display()
        ))
    })?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(fail(format!(
            "refusing unsafe Muse executable: {}",
            resolved.display()
        )));
    }
    Ok(resolved)
}

fn output_detail(output: &[u8]) -> String {
    let start = output.len().saturating_sub(4000);
    String::from_utf8_lossy(&output[start..]).trim().to_owned()
}

fn install_muse(force: bool) -> Result<&'static str, AgentError> {
    // Resolve and validate code before making any configuration directories.
    let executable = muse_executable()?;
    let config_home = muse_config_home()?;
    let skills = config_home.join("skills");
    ensure_real_directory(&skills)?;
    let destination_directory = skills.join("agentctl");
    if let Ok(metadata) = fs::symlink_metadata(&destination_directory) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(fail(format!(
                "refusing non-directory skill destination: {}",
                destination_directory.display()
            )));
        }
    }
    let destination = destination_directory.join("SKILL.md");
    if let Some(outcome) = existing(&destination, force)? {
        return Ok(outcome);
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| fail(error.to_string()))?
        .as_nanos();
    let staging = std::env::temp_dir().join(format!(
        "agentctl-muse-skill.{}.{nonce}",
        std::process::id()
    ));
    DirBuilder::new()
        .mode(0o700)
        .create(&staging)
        .map_err(|error| fail(format!("cannot stage Muse skill: {error}")))?;
    let source = staging.join("agentctl");
    DirBuilder::new()
        .mode(0o700)
        .create(&source)
        .map_err(|error| fail(format!("cannot stage Muse skill: {error}")))?;
    let source_file = source.join("SKILL.md");
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&source_file)?;
        file.write_all(crate::AGENTCTL_SKILL.as_bytes())?;
        file.sync_all()?;
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_dir_all(&staging);
        return Err(fail(format!("cannot stage Muse skill: {error}")));
    }

    let mut command = Command::new(executable);
    command
        .args(["skills", "install"])
        .arg(&source)
        .args(["--scope", "user", "--name", "agentctl", "--json"])
        .env(
            "XDG_CONFIG_HOME",
            config_home.parent().expect("Muse config parent"),
        )
        .stdin(Stdio::null());
    if force {
        command.arg("--force");
    }
    let output = crate::client::bounded_output(command, Duration::from_secs(30));
    let _ = fs::remove_dir_all(&staging);
    let output =
        output.map_err(|error| fail(format!("Muse skill installation failed: {error}")))?;
    if !output.status.success() {
        let detail = if output.stderr.is_empty() {
            output_detail(&output.stdout)
        } else {
            output_detail(&output.stderr)
        };
        return Err(fail(format!(
            "Muse skill installation failed: {}",
            if detail.is_empty() {
                output.status.to_string()
            } else {
                detail
            }
        )));
    }
    let receipt: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        fail(format!(
            "Muse skill installation returned invalid JSON: {error}"
        ))
    })?;
    if receipt
        .get("installed")
        .and_then(Value::as_object)
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        != Some("agentctl")
    {
        return Err(fail("Muse skill installation returned no agentctl receipt"));
    }
    if existing(&destination, false)? != Some("unchanged") {
        return Err(fail(
            "Muse reported installation without the expected managed skill",
        ));
    }
    Ok("installed")
}

pub(crate) fn install(harnesses: &[String], force: bool) -> Result<InstallResult, AgentError> {
    let selected = if harnesses.is_empty() {
        vec!["codex".to_owned(), "claude".to_owned(), "muse".to_owned()]
    } else {
        let mut values = Vec::new();
        for harness in harnesses {
            if !matches!(harness.as_str(), "codex" | "claude" | "muse") {
                return Err(fail(format!("unsupported skill harness {harness:?}")));
            }
            if !values.contains(harness) {
                values.push(harness.clone());
            }
        }
        values
    };
    let mut result = InstallResult {
        installed: Vec::new(),
        unchanged: Vec::new(),
    };
    for harness in selected {
        let outcome = if harness == "muse" {
            install_muse(force)?
        } else {
            let destination = direct_root(&harness)?.join("agentctl/SKILL.md");
            write_direct(&destination, force)?
        };
        match outcome {
            "installed" => result.installed.push(harness),
            "unchanged" => result.unchanged.push(harness),
            _ => unreachable!(),
        }
    }
    Ok(result)
}
