//! Read-only disk pressure observations and durable, rate-limited coordinator notices.
use super::*;

/// One opt-in filesystem pressure check. Thresholds are bytes (GB = 1,000,000,000 bytes).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiskWatch {
    /// Absolute path on the filesystem to measure; df reports bytes available to this user.
    pub path: PathBuf,
    /// Below this threshold, report a warning in health.json and the journal.
    pub warn_below_bytes: u64,
    /// Below this threshold, queue a coordinator alert, at most once per hour per path.
    pub alert_below_bytes: u64,
    /// Optional operator-configured read-only diagnostic argv, never shell-evaluated.
    /// Its output (up to 4096 characters) is attached to due alerts; deadline 30 seconds.
    /// Use the host's slot manager to report its largest target directories.
    #[serde(default)]
    pub usage_command: Vec<String>,
}

impl DiskWatch {
    pub(super) fn validate(&self) -> std::result::Result<(), String> {
        if !self.path.is_absolute()
            || self.alert_below_bytes == 0
            || self.warn_below_bytes <= self.alert_below_bytes
        {
            return Err(
                "disks need an absolute path and warn_below_bytes > alert_below_bytes > 0".into(),
            );
        }
        if let Some(program) = self.usage_command.first() {
            if !Path::new(program).is_absolute()
                || self.usage_command.iter().any(|arg| arg.contains('\0'))
            {
                return Err(
                    "disk usage_command needs an absolute program and NUL-free argv".into(),
                );
            }
        }
        Ok(())
    }

    pub(super) fn check(&self, host: &dyn Host) -> Value {
        let result = host.run(
            &[
                "/bin/df".into(),
                "--block-size=1".into(),
                "--output=avail".into(),
                "--".into(),
                self.path.to_string_lossy().into_owned(),
            ],
            None,
            Duration::from_secs(5),
        );
        let free = result
            .stdout
            .lines()
            .last()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let Some(free) = free.filter(|_| result.status == Some(0)) else {
            return json!({"check":"disk", "subject":self.path, "ok":false,
                "severity":"unknown", "repair":null,
                "detail":format!("cannot measure free space (status {:?}): {}", result.status,
                    result.stderr.chars().take(512).collect::<String>())});
        };
        let critical = free < self.alert_below_bytes;
        let warning = free < self.warn_below_bytes;
        let mut row = json!({"check":"disk", "subject":self.path, "ok":!critical,
            "severity":if critical { "critical" } else if warning { "warning" } else { "healthy" },
            "available_bytes":free, "warn_below_bytes":self.warn_below_bytes,
            "alert_below_bytes":self.alert_below_bytes, "repair":null,
            "detail":format!("{:.2} GB free; warn below {:.2} GB, alert below {:.2} GB; no automatic deletion",
                free as f64 / 1e9, self.warn_below_bytes as f64 / 1e9, self.alert_below_bytes as f64 / 1e9)});
        if warning && !critical {
            row["warning"] = json!("low-disk-space");
        }
        row
    }

    fn usage(&self, host: &dyn Host) -> String {
        if self.usage_command.is_empty() {
            return "usage diagnostic not configured".into();
        }
        let result = host.run(&self.usage_command, None, Duration::from_secs(30));
        if result.status == Some(0) && !result.stdout.trim().is_empty() {
            result.stdout.chars().take(4096).collect()
        } else {
            format!(
                "usage diagnostic unavailable (status {:?}): {}",
                result.status,
                result.stderr.chars().take(512).collect::<String>()
            )
        }
    }
}

/// Use the existing agent queue and persisted alert timestamps. Never type or remove files.
pub(super) fn alert<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    config: &RestoreConfig,
    host: &dyn Host,
    findings: &[Value],
    alerted: &mut BTreeMap<String, f64>,
) -> Option<Value> {
    let now = host.now();
    let due: Vec<_> = findings
        .iter()
        .filter(|row| row["check"] == "disk" && row["ok"] == false)
        .filter(|row| {
            alerted
                .get(&format!("disk:{}", row["subject"].as_str().unwrap_or("")))
                .is_none_or(|last| now - last >= REALERT_SECONDS)
        })
        .collect();
    if due.is_empty() {
        return None;
    }
    let Some(name) = config.coordinator.as_deref() else {
        return Some(
            json!({"sent":false,"queued":false,"reason":"disk alert has no configured coordinator"}),
        );
    };
    let lines: Vec<_> = due
        .iter()
        .map(|row| {
            let path = row["subject"].as_str().unwrap_or("");
            let usage = config
                .disks
                .iter()
                .find(|disk| disk.path == Path::new(path))
                .map(|disk| disk.usage(host))
                .unwrap_or_default();
            format!(
                "- {path}: {}\n{usage}",
                row["detail"].as_str().unwrap_or("")
            )
        })
        .collect();
    let id = format!("watch-disk-{}", (now * 1000.0) as u64);
    let text = format!(
        "[agentctl watch] Disk pressure needs attention:\n{}",
        lines.join("\n")
    );
    let result = ManagedAgents::new(client, registry).and_then(|manager| {
        manager.queue_identified_with_lock_timeout(
            name,
            &text,
            None,
            None,
            Some(&id),
            Some(Duration::from_secs(1)),
        )
    });
    match result {
        Ok(result) => {
            let queued = result.pending.contains(&id);
            if queued {
                for row in due {
                    alerted.insert(
                        format!("disk:{}", row["subject"].as_str().unwrap_or("")),
                        now,
                    );
                }
            }
            Some(
                json!({"sent":false,"queued":queued,"message_id":id,"outcome":json!(result)["outcome"]}),
            )
        }
        Err(error) => Some(json!({"sent":false,"queued":false,"reason":error.to_string()})),
    }
}
