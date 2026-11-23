//! Owner-declared off-limits path masking (#2163 agentctl-profile-masked-paths).
//!
//! A launched agent runs inside a nested user and mount namespace: the
//! outer namespace bind-mounts an empty read-only directory over each
//! masked path, then the harness execs through a second user-namespace
//! step that restores the real uid and gid. Masking is fail-closed: any
//! setup failure refuses the launch rather than launching unmasked.

use std::path::{Component, Path, PathBuf};

/// Validate a declared masked-path list: absolute, NUL-free, deduplicated.
pub(crate) fn validate_masked_paths(values: &[String], label: &str) -> Result<Vec<String>, String> {
    let mut result: Vec<String> = Vec::new();
    for value in values {
        if value.is_empty() || value.contains('\0') || !Path::new(value).is_absolute() {
            return Err(format!(
                "{label} masked_paths entries must be nonempty absolute paths, not {value:?}"
            ));
        }
        // Reject `.`/`..` so a declared path names one exact location.
        if Path::new(value)
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err(format!(
                "{label} masked_paths entries must not contain . or .. components: {value:?}"
            ));
        }
        if !result.contains(value) {
            result.push(value.clone());
        }
    }
    Ok(result)
}

/// Parse a profile's raw `masked_paths` JSON value (an array of strings).
pub(crate) fn masked_paths_from_value(
    value: Option<&serde_json::Value>,
    label: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(format!(
            "{label} masked_paths must be an array of absolute paths"
        ));
    };
    let mut raw = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(format!(
                "{label} masked_paths must be an array of absolute paths"
            ));
        };
        raw.push(text.to_owned());
    }
    validate_masked_paths(&raw, label)
}

/// The effective masks: a per-agent override wins over the profile's
/// declared list; with no override the declared list applies.
pub(crate) fn effective_masked_paths(
    declared: &[String],
    override_paths: Option<&[String]>,
) -> Vec<String> {
    match override_paths {
        Some(paths) => paths.to_vec(),
        None => declared.to_vec(),
    }
}

/// Real uid/gid of this process, captured before any namespace entry.
pub(crate) fn real_ids() -> (u32, u32) {
    // SAFETY: geteuid/getegid cannot fail.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Mount script shared by the pane entry and the headless wrapper: cover
/// every masked path with an empty read-only directory. Any failure exits
/// nonzero so the caller refuses the launch (fail closed).
fn mount_script(masked: &[String]) -> String {
    let mut script = String::from("empty_dir=$(mktemp -d) && chmod 555 \"$empty_dir\"");
    for path in masked {
        script.push_str(&format!(
            " && mount --bind \"$empty_dir\" {} && mount -o remount,ro,bind {}",
            shell_quote(path),
            shell_quote(path)
        ));
    }
    script
}

/// Pane entry line (#2163): replaces the pane shell with a shell inside
/// the mask namespace, keeping the same PID through `exec` at each step.
/// The harness started afterwards in this pane inherits the namespace.
pub(crate) fn masked_shell_line(masked: &[String], uid: u32, gid: u32) -> String {
    let inner = format!(
        "{} && exec unshare --user --map-user={uid} --map-group={gid} /bin/sh",
        mount_script(masked)
    );
    format!("exec unshare -r -m sh -c {}", shell_quote(&inner))
}

/// Wrap a harness command for a headless launch: the returned argv runs
/// the namespace setup, then execs the original command with the real
/// uid/gid. Empty `masked` returns the command unchanged.
pub(crate) fn wrap_command(masked: &[String], command: Vec<String>) -> Vec<String> {
    if masked.is_empty() || command.is_empty() {
        return command;
    }
    let (uid, gid) = real_ids();
    let inner = format!(
        "{} && exec unshare --user --map-user={uid} --map-group={gid} \"$@\"",
        mount_script(masked)
    );
    let mut wrapped = vec![
        "unshare".to_owned(),
        "-r".to_owned(),
        "-m".to_owned(),
        "sh".to_owned(),
        "-c".to_owned(),
        inner,
        "sh".to_owned(),
    ];
    wrapped.extend(command);
    wrapped
}

/// Whether `cwd` is a masked path or lives under one. Both sides are
/// canonicalized when possible so a symlinked spelling still matches.
pub(crate) fn path_is_masked(cwd: &Path, masked: &[String]) -> bool {
    if masked.is_empty() {
        return false;
    }
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    masked.iter().any(|entry| {
        let base: PathBuf = std::fs::canonicalize(entry).unwrap_or_else(|_| PathBuf::from(entry));
        cwd == base || cwd.starts_with(&base)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn masked_paths_must_be_absolute_and_clean() {
        assert!(validate_masked_paths(&strings(&["/off/limits"]), "profile \"x\"").is_ok());
        let relative = validate_masked_paths(&strings(&["relative/path"]), "profile \"x\"")
            .expect_err("relative masked path refused");
        assert!(relative.contains("absolute"), "{relative}");
        let dotted = validate_masked_paths(&strings(&["/off/../limits"]), "profile \"x\"")
            .expect_err("dotted masked path refused");
        assert!(dotted.contains(".."), "{dotted}");
        let deduped =
            validate_masked_paths(&strings(&["/a", "/a", "/b"]), "profile \"x\"").unwrap();
        assert_eq!(deduped, strings(&["/a", "/b"]));
    }

    #[test]
    fn per_agent_override_wins_over_profile() {
        let declared = strings(&["/declared"]);
        assert_eq!(effective_masked_paths(&declared, None), declared);
        assert_eq!(
            effective_masked_paths(&declared, Some(&strings(&["/override"]))),
            strings(&["/override"])
        );
        // An explicit empty override unmasks: the record wins, even to none.
        assert!(effective_masked_paths(&declared, Some(&[])).is_empty());
    }

    #[test]
    fn wrapped_command_names_every_masked_path() {
        let wrapped = wrap_command(&strings(&["/off/limits"]), strings(&["harness", "--flag"]));
        assert_eq!(wrapped[0], "unshare");
        let script = &wrapped[5];
        assert!(script.contains("/off/limits"), "{script}");
        assert!(script.contains("--map-user="), "{script}");
        assert_eq!(wrapped[7], "harness");
        assert_eq!(wrapped[8], "--flag");
        // No masks: the command is untouched.
        assert_eq!(
            wrap_command(&[], strings(&["harness"])),
            strings(&["harness"])
        );
    }

    #[test]
    fn masked_shell_line_covers_and_restores_identity() {
        let line = masked_shell_line(&strings(&["/off/limits"]), 1000, 100);
        assert!(line.starts_with("exec unshare -r -m sh -c "), "{line}");
        assert!(line.contains("/off/limits"), "{line}");
        assert!(line.contains("--map-user=1000"), "{line}");
        assert!(line.contains("--map-group=100"), "{line}");
    }

    #[test]
    fn cwd_under_a_masked_path_is_detected() {
        let root =
            std::env::temp_dir().join(format!("agentctl-masked-test-{}", std::process::id()));
        let secret = root.join("secret");
        let inside = secret.join("deep");
        std::fs::create_dir_all(&inside).unwrap();
        let masked = vec![secret.display().to_string()];
        assert!(path_is_masked(&inside, &masked));
        assert!(path_is_masked(&secret, &masked));
        assert!(!path_is_masked(&root, &masked));
        assert!(!path_is_masked(&inside, &[]));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Hermetic end-to-end proof: a temporary directory is the masked
    /// path, and the wrapped command cannot see its contents while
    /// running with the caller's real uid. Skips only when this host
    /// cannot create a user namespace at all.
    #[test]
    fn wrapped_process_cannot_see_the_masked_path() {
        let preflight = std::process::Command::new("unshare")
            .args(["-r", "-m", "true"])
            .status();
        if !matches!(preflight, Ok(status) if status.success()) {
            return;
        }
        let root = std::env::temp_dir().join(format!("agentctl-masked-e2e-{}", std::process::id()));
        let secret = root.join("secret");
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("data.txt"), "off-limits").unwrap();
        let masked = vec![secret.display().to_string()];
        let wrapped = wrap_command(
            &masked,
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                format!(
                    "echo UID=$(id -u); echo LIST-BEGIN; ls {}; echo LIST-END; cat {}/data.txt 2>/dev/null || echo CAT-REFUSED",
                    secret.display(),
                    secret.display()
                ),
            ],
        );
        let output = std::process::Command::new(&wrapped[0])
            .args(&wrapped[1..])
            .output()
            .expect("run wrapped command");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // Real uid, empty listing, and the secret never readable.
        assert!(text.contains(&format!("UID={}", real_ids().0)), "{text}");
        assert!(text.contains("LIST-BEGIN\nLIST-END"), "{text}");
        assert!(text.contains("CAT-REFUSED"), "{text}");
        assert!(!text.contains("off-limits"), "{text}");
        // The host view is untouched.
        assert_eq!(
            std::fs::read_to_string(secret.join("data.txt")).unwrap(),
            "off-limits"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
