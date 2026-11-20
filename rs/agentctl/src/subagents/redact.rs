//! Rewrite one literal string out of one agent's retained history (#1963).

use super::*;

const REPLACEMENT: &str = "[redacted]";

/// How many times `literal` occurs inside JSON string values of `value`.
fn count_in(value: &Value, literal: &str) -> usize {
    match value {
        Value::String(text) => text.match_indices(literal).count(),
        Value::Array(items) => items.iter().map(|item| count_in(item, literal)).sum(),
        Value::Object(map) => map.values().map(|item| count_in(item, literal)).sum(),
        _ => 0,
    }
}

/// Replace `literal` inside JSON string values only; returns the count.
fn replace_in(value: &mut Value, literal: &str) -> usize {
    match value {
        Value::String(text) => {
            let count = text.match_indices(literal).count();
            if count > 0 {
                *text = text.replace(literal, REPLACEMENT);
            }
            count
        }
        Value::Array(items) => items.iter_mut().map(|item| replace_in(item, literal)).sum(),
        Value::Object(map) => map.values_mut().map(|item| replace_in(item, literal)).sum(),
        _ => 0,
    }
}

/// Sorted regular files of `dir` with one extension; empty when it is absent.
fn files_with(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.to_string_lossy() == extension)
        })
        .collect();
    files.sort();
    files
}

impl<C: ManagedApi + ?Sized> ManagedAgents<'_, C> {
    /// Rewrite `literal` to `[redacted]` across one agent's processed queue
    /// and events, and (with `include_archived`) its archived records'
    /// processed queues (#1963). Never touches the live inbox or in-flight
    /// entries, and refuses while the agent is mid-delivery (an inflight
    /// entry exists). Each touched file is first backed up (0600) under
    /// `REGISTRY/backups/`; an audit line naming who, when, the files and
    /// the match count — never the literal — is appended there.
    pub fn redact(
        &self,
        agent_name: &str,
        literal: &str,
        include_archived: bool,
        dry_run: bool,
    ) -> Result<Value> {
        if literal.is_empty() {
            return Err(fail("redact literal must not be empty"));
        }
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        let _ = record;
        let agent_dir = self.directory(agent_name)?;
        let inflight = agent_dir.join("queue").join("inflight");
        let in_flight = fs::read_dir(&inflight).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        });
        if in_flight {
            return Err(fail(format!(
                "agent {agent_name:?} is mid-delivery (an inflight entry exists); refusing to redact — remedy: wait for the delivery to finish, then retry"
            )));
        }
        let mut targets: Vec<PathBuf> = Vec::new();
        targets.extend(files_with(
            &agent_dir.join("queue").join("processed"),
            "json",
        ));
        targets.extend(files_with(&agent_dir.join("events"), "jsonl"));
        if include_archived {
            let archive = self.registry.join("archive");
            let prefix = format!("{agent_name}-");
            if let Ok(entries) = fs::read_dir(&archive) {
                let mut dirs: Vec<PathBuf> = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.is_dir()
                            && path
                                .file_name()
                                .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
                    })
                    .collect();
                dirs.sort();
                for dir in dirs {
                    targets.extend(files_with(&dir.join("queue").join("processed"), "json"));
                    targets.extend(files_with(&dir.join("events"), "jsonl"));
                }
            }
        }
        // Count first, so a file that cannot be rewritten changes nothing.
        let mut planned: Vec<(PathBuf, usize, Vec<String>)> = Vec::new();
        for path in &targets {
            let text = fs::read_to_string(path)
                .map_err(|error| fail(format!("cannot read {}: {error}", path.display())))?;
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let mut total = 0;
                let mut lines = Vec::new();
                for line in text.lines() {
                    match serde_json::from_str::<Value>(line) {
                        Ok(mut value) => {
                            let count = replace_in(&mut value, literal);
                            total += count;
                            lines.push(if count > 0 {
                                serde_json::to_string(&value)
                                    .map_err(|error| fail(error.to_string()))?
                            } else {
                                line.to_owned()
                            });
                        }
                        Err(_) => lines.push(line.to_owned()),
                    }
                }
                if total > 0 {
                    planned.push((path.clone(), total, lines));
                }
            } else {
                let mut value: Value = serde_json::from_str(&text).map_err(|_| {
                    fail(format!(
                        "cannot redact {}: it is not valid JSON",
                        path.display()
                    ))
                })?;
                let total = count_in(&value, literal);
                if total > 0 {
                    replace_in(&mut value, literal);
                    planned.push((
                        path.clone(),
                        total,
                        vec![serde_json::to_string(&value)
                            .map_err(|error| fail(error.to_string()))?],
                    ));
                }
            }
        }
        let total_matches: usize = planned.iter().map(|(_, count, _)| count).sum();
        let file_rows: Vec<Value> = planned
            .iter()
            .map(|(path, count, _)| {
                json!({
                    "file": path.strip_prefix(&self.registry).unwrap_or(path).display().to_string(),
                    "matches": count,
                })
            })
            .collect();
        if dry_run {
            return Ok(json!({
                "name": agent_name,
                "dry_run": true,
                "files": file_rows,
                "total_matches": total_matches,
            }));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_secs();
        let backups_root = self.registry.join("backups");
        let mut backup_dir = backups_root.join(format!("redact-{agent_name}-{now}"));
        let mut suffix = 0;
        while backup_dir.exists() {
            suffix += 1;
            backup_dir = backups_root.join(format!("redact-{agent_name}-{now}-{suffix}"));
        }
        let mut written: Vec<String> = Vec::new();
        if !planned.is_empty() {
            fs::create_dir_all(&backup_dir).map_err(|error| fail(error.to_string()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&backups_root, fs::Permissions::from_mode(0o700));
                let _ = fs::set_permissions(&backup_dir, fs::Permissions::from_mode(0o700));
            }
            for (index, (path, _, lines)) in planned.iter().enumerate() {
                let relative = path
                    .strip_prefix(&self.registry)
                    .unwrap_or(path)
                    .display()
                    .to_string();
                let backup = backup_dir.join(format!("{index:04}.bak"));
                let original = fs::read(path).map_err(|error| fail(error.to_string()))?;
                fs::write(&backup, &original).map_err(|error| fail(error.to_string()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))
                        .map_err(|error| fail(error.to_string()))?;
                }
                let mut content = lines.join("\n");
                if path.extension().is_some_and(|ext| ext == "jsonl") && !content.is_empty() {
                    content.push('\n');
                }
                fs::write(path, content).map_err(|error| fail(error.to_string()))?;
                written.push(relative);
            }
            // The audit line lives beside the backups and never holds the literal.
            let who = std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned());
            let audit = json!({
                "schema": "agentctl-redact-audit/v1",
                "agent": agent_name,
                "who": who,
                "at": now,
                "files": written,
                "total_matches": total_matches,
                "backup": backup_dir
                    .strip_prefix(&self.registry)
                    .unwrap_or(&backup_dir)
                    .display()
                    .to_string(),
            });
            let audit_path = backups_root.join("redact-audit.jsonl");
            let mut line =
                serde_json::to_string(&audit).map_err(|error| fail(error.to_string()))?;
            line.push('\n');
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&audit_path)
                .map_err(|error| fail(error.to_string()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&audit_path, fs::Permissions::from_mode(0o600));
            }
            use std::io::Write;
            file.write_all(line.as_bytes())
                .map_err(|error| fail(error.to_string()))?;
        }
        Ok(json!({
            "name": agent_name,
            "dry_run": false,
            "files": file_rows,
            "total_matches": total_matches,
            "backup": if planned.is_empty() { Value::Null } else {
                json!(backup_dir.strip_prefix(&self.registry).unwrap_or(&backup_dir).display().to_string())
            },
        }))
    }
}
