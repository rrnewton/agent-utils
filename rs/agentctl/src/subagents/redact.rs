//! Rewrite one literal string out of one agent's retained history (#1963).

use super::*;

const REPLACEMENT: &str = "[redacted]";
const COMMON_WORDS: &[&str] = &[
    "true", "false", "null", "idle", "running", "stopped", "unknown", "string", "number", "object",
    "array", "text", "message", "agent", "session", "token", "type", "state", "status", "name",
    "value",
];

/// Areas a redact never rewrites, named in every result (review item 11).
pub(super) const UNCOVERED: &[&str] = &[
    "queue/inbox and queue/inflight (live delivery state)",
    "queue/failed and queue/cancelled",
    "agent.json (the registry record)",
    "misroutes.jsonl and readback.jsonl",
    "chat bridge state",
    "harness transcripts and session files (clean those with the harness's own tooling)",
];

fn validate_literal(literal: &str) -> Result<()> {
    if literal.chars().count() < 12 {
        return Err(fail(
            "redact literal must be at least 12 characters — remedy: pass a longer, specific string (a full token or id), not a short fragment",
        ));
    }
    if COMMON_WORDS.contains(&literal.to_lowercase().as_str())
        || literal.parse::<f64>().is_ok()
        || REPLACEMENT.contains(literal)
        || literal.contains(REPLACEMENT)
    {
        return Err(fail(
            "redact literal is a common JSON word or the replacement itself — remedy: pass the specific string to remove, not a JSON keyword, number, or [redacted]",
        ));
    }
    Ok(())
}

fn count_in(value: &Value, literal: &str) -> usize {
    match value {
        Value::String(text) => text.match_indices(literal).count(),
        Value::Array(items) => items.iter().map(|item| count_in(item, literal)).sum(),
        Value::Object(map) => map.values().map(|item| count_in(item, literal)).sum(),
        _ => 0,
    }
}

fn count_keys(value: &Value, literal: &str) -> usize {
    match value {
        Value::Array(items) => items.iter().map(|item| count_keys(item, literal)).sum(),
        Value::Object(map) => map
            .iter()
            .map(|(key, item)| key.match_indices(literal).count() + count_keys(item, literal))
            .sum(),
        _ => 0,
    }
}

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

/// Archive directories named exactly `<name>-<digits>-<digits>` (review item 7).
fn archive_dirs(registry: &Path, agent_name: &str) -> Vec<PathBuf> {
    let archive = registry.join("archive");
    let Ok(entries) = fs::read_dir(&archive) else {
        return Vec::new();
    };
    let prefix = format!("{agent_name}-");
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name.strip_prefix(&prefix).is_some_and(|rest| {
                        let mut parts = rest.split('-');
                        matches!(parts.next(), Some(a) if !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit()))
                            && matches!(parts.next(), Some(b) if !b.is_empty() && b.bytes().all(|b| b.is_ascii_digit()))
                            && parts.next().is_none()
                    })
                })
        })
        .collect();
    dirs.sort();
    dirs
}

/// Write `bytes` over `path` atomically: temp in the same directory, sync, rename.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| fail("redact target has no parent"))?;
    let temporary = parent.join(format!(
        ".redact-{}-{}.tmp",
        std::process::id(),
        path.as_os_str().len()
    ));
    let write = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    if write.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write.map_err(|error| fail(format!("cannot write {}: {error}", path.display())))
}

struct Planned {
    path: PathBuf,
    matches: usize,
    json: Option<Value>,
    lines: Option<Vec<String>>,
}

impl<C: ManagedApi + ?Sized> ManagedAgents<'_, C> {
    /// Rewrite `literal` to `[redacted]` across one agent's processed queue
    /// and events (#1963). See the CLI help for what stays uncovered.
    #[allow(clippy::too_many_arguments)]
    pub fn redact(
        &self,
        agent_name: &str,
        literal: &str,
        include_archived: bool,
        dry_run: bool,
        no_backup: bool,
    ) -> Result<Value> {
        validate_literal(literal)?;
        // Resolve aliases exactly as a load does (review item 8), and support
        // retired agents whose record survives only in the archive (item 9).
        let (record_name, agent_dir, stopped) = match self.load(agent_name) {
            Ok(record) => {
                let dir = self.directory(&record.name)?;
                let stopped = record.lifecycle == "stopped";
                (record.name.clone(), dir, stopped)
            }
            Err(_) => {
                let dirs = archive_dirs(&self.registry, agent_name);
                let Some(dir) = dirs.last().cloned() else {
                    return Err(fail(format!("unknown agent {agent_name:?}")));
                };
                (agent_name.to_owned(), dir, true)
            }
        };
        let _lock = self.lock(&record_name)?;
        let _queue = self.queue_locks(&record_name).ok();
        let inflight = agent_dir.join("queue").join("inflight");
        let in_flight = fs::read_dir(&inflight).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        });
        if in_flight {
            return Err(fail(format!(
                "agent {record_name:?} is mid-delivery (an inflight entry exists); refusing to redact — remedy: wait for the delivery to finish, then retry"
            )));
        }
        let mut targets: Vec<PathBuf> = Vec::new();
        targets.extend(files_with(
            &agent_dir.join("queue").join("processed"),
            "json",
        ));
        targets.extend(files_with(&agent_dir.join("events"), "jsonl"));
        if include_archived
            && agent_dir
                .parent()
                .is_some_and(|p| p.file_name().is_some_and(|n| n != "archive"))
        {
            for dir in archive_dirs(&self.registry, &record_name) {
                targets.extend(files_with(&dir.join("queue").join("processed"), "json"));
                targets.extend(files_with(&dir.join("events"), "jsonl"));
            }
        }
        let mut planned: Vec<Planned> = Vec::new();
        let mut left_keys = 0usize;
        let mut left_unparseable = 0usize;
        for path in &targets {
            let bytes = fs::read(path)
                .map_err(|error| fail(format!("cannot read {}: {error}", path.display())))?;
            let text = String::from_utf8(bytes).map_err(|_| {
                fail(format!(
                    "cannot redact {}: it is not valid UTF-8",
                    path.display()
                ))
            })?;
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let mut total = 0;
                let mut lines = Vec::new();
                for line in text.split('\n') {
                    if line.is_empty() {
                        lines.push(line.to_owned());
                        continue;
                    }
                    match serde_json::from_str::<Value>(line) {
                        Ok(mut value) => {
                            left_keys += count_keys(&value, literal);
                            let count = replace_in(&mut value, literal);
                            total += count;
                            lines.push(if count > 0 {
                                serde_json::to_string(&value)
                                    .map_err(|error| fail(error.to_string()))?
                            } else {
                                line.to_owned()
                            });
                        }
                        Err(_) => {
                            left_unparseable += line.match_indices(literal).count();
                            lines.push(line.to_owned());
                        }
                    }
                }
                if total > 0 {
                    planned.push(Planned {
                        path: path.clone(),
                        matches: total,
                        json: None,
                        lines: Some(lines),
                    });
                }
            } else {
                let mut value: Value = serde_json::from_str(&text).map_err(|_| {
                    fail(format!(
                        "cannot redact {}: it is not valid JSON",
                        path.display()
                    ))
                })?;
                left_keys += count_keys(&value, literal);
                let total = count_in(&value, literal);
                if total > 0 {
                    replace_in(&mut value, literal);
                    planned.push(Planned {
                        path: path.clone(),
                        matches: total,
                        json: Some(value),
                        lines: None,
                    });
                }
            }
        }
        // Event logs are append-only while the wrapper lives: never rewrite
        // them unless the agent is stopped (review blocker 1).
        if !dry_run
            && !stopped
            && planned
                .iter()
                .any(|item| item.path.extension().is_some_and(|ext| ext == "jsonl"))
        {
            return Err(fail(format!(
                "agent {record_name:?} is not stopped and its events would be rewritten; the event log is appended live — remedy: stop the agent first (`agentctl stop {record_name}`), then redact"
            )));
        }
        let total_matches: usize = planned.iter().map(|item| item.matches).sum();
        let file_rows: Vec<Value> = planned
            .iter()
            .map(|item| json!({
                "file": item.path.strip_prefix(&self.registry).unwrap_or(&item.path).display().to_string(),
                "matches": item.matches,
            }))
            .collect();
        let result = |backup: Value| {
            json!({
                "name": record_name,
                "dry_run": dry_run,
                "files": file_rows,
                "total_matches": total_matches,
                "left_behind": {"keys": left_keys, "unparseable_lines": left_unparseable},
                "clean": left_keys == 0 && left_unparseable == 0,
                "backup": backup,
                "uncovered": UNCOVERED,
            })
        };
        if dry_run || planned.is_empty() {
            return Ok(result(Value::Null));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_secs();
        let who = std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned());
        let backups_root = self.registry.join("backups");
        let audit_path = backups_root.join("redact-audit.jsonl");
        let append_audit = |phase: &str, backup: &str| -> Result<()> {
            fs::create_dir_all(&backups_root).map_err(|error| fail(error.to_string()))?;
            let audit = json!({
                "schema": "agentctl-redact-audit/v1", "phase": phase,
                "agent": record_name, "who": who, "at": now,
                "files": planned.iter().map(|item| item.path.strip_prefix(&self.registry).unwrap_or(&item.path).display().to_string()).collect::<Vec<_>>(),
                "total_matches": total_matches, "backup": backup,
            });
            let mut line =
                serde_json::to_string(&audit).map_err(|error| fail(error.to_string()))?;
            line.push('\n');
            use std::io::Write;
            let mut options = fs::OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&audit_path)
                .map_err(|error| fail(error.to_string()))?;
            file.write_all(line.as_bytes())
                .map_err(|error| fail(error.to_string()))
        };
        // Backups hold the literal: create them 0600 from the first byte and
        // fail when the directory cannot be made private (review item 12).
        let backup_name = if no_backup {
            append_audit("started", "none (--no-backup)")?;
            String::new()
        } else {
            let mut name = format!("redact-{record_name}-{now}");
            let mut suffix = 0;
            while backups_root.join(&name).exists() {
                suffix += 1;
                name = format!("redact-{record_name}-{now}-{suffix}");
            }
            let dir = backups_root.join(&name);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&backups_root)
                    .or_else(|error| {
                        if error.kind() == std::io::ErrorKind::AlreadyExists {
                            Ok(())
                        } else {
                            Err(error)
                        }
                    })
                    .map_err(|error| fail(format!("cannot create backups directory: {error}")))?;
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&dir)
                    .map_err(|error| fail(format!("cannot create backup directory: {error}")))?;
            }
            #[cfg(not(unix))]
            fs::create_dir_all(&dir).map_err(|error| fail(error.to_string()))?;
            append_audit("started", &name)?;
            name
        };
        let backup_dir = backups_root.join(&backup_name);
        for (index, item) in planned.iter().enumerate() {
            if !no_backup {
                let backup = backup_dir.join(format!("{index:04}.bak"));
                let original = fs::read(&item.path).map_err(|error| fail(error.to_string()))?;
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                use std::io::Write;
                let mut file = options.open(&backup).map_err(|error| {
                    fail(format!(
                        "cannot create backup {}: {error}",
                        backup.display()
                    ))
                })?;
                file.write_all(&original)
                    .map_err(|error| fail(error.to_string()))?;
            }
            match (&item.json, &item.lines) {
                (Some(value), _) => agent::atomic_json(&item.path, value)?,
                (_, Some(lines)) => {
                    let mut content = lines.join("\n");
                    if !content.ends_with('\n') {
                        content.push('\n');
                    }
                    atomic_write(&item.path, content.as_bytes())?;
                }
                _ => unreachable!(),
            }
        }
        append_audit(
            "completed",
            if no_backup {
                "none (--no-backup)"
            } else {
                &backup_name
            },
        )?;
        Ok(result(if no_backup {
            Value::Null
        } else {
            json!(format!("backups/{backup_name}"))
        }))
    }
}
