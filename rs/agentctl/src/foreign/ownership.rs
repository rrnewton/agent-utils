//! Generation-safe ownership identity types.
//!
//! A process number names a process only until that process is reaped,
//! so every recorded identity is the triple `(boot_id, pid, start ticks)`.
//!
//! This module covers record types and read/validate: [`ProcessIdentity`],
//! [`HarnessRecord`], [`parse_harness_record`], and [`HarnessRecord::document`],
//! plus the kernel evidence those records are checked against:
//! [`current_boot_id`], [`read_stat`], and [`scan_processes`]. Process pinning
//! via pidfds, harness-tree cleanup, and cgroup containment live elsewhere.

use std::fmt;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use super::procstat::{parse_process_stat, ProcessStat};

/// Env var carrying a turn's harness generation into the harness process.
pub const GENERATION_ENV: &str = "AGENTCTL_HARNESS_GENERATION";
/// Schema tag written into `active-harness.json`.
pub const HARNESS_SCHEMA: &str = "agentctl-headless-harness/v1";
/// Directory-name prefix for per-turn cgroup v2 containment.
pub const CGROUP_PREFIX: &str = "agentctl-harness-";

const MAX_PROCESS_ID: i64 = (1 << 31) - 1;

/// Structured operation error with a machine-stable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignOwnershipError {
    /// Machine-stable error code (e.g. `harness_identity_unknown`).
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
}

impl ForeignOwnershipError {
    fn harness_identity_unknown(detail: &str) -> Self {
        Self {
            code: "harness_identity_unknown",
            message: format!("malformed active harness identity: {detail}"),
        }
    }

    fn runner_identity_unknown(message: String) -> Self {
        Self {
            code: "runner_identity_unknown",
            message,
        }
    }
}

impl fmt::Display for ForeignOwnershipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ForeignOwnershipError {}

/// How one turn's harness tree is contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    /// Tracked by process/session proofs only.
    Process,
    /// Tracked by a per-turn cgroup v2 directory.
    Cgroup,
    /// Written before boot identities and generations existed.
    Legacy,
}

impl Containment {
    fn as_str(self) -> &'static str {
        match self {
            Containment::Process => "process",
            Containment::Cgroup => "cgroup",
            Containment::Legacy => "legacy",
        }
    }
}

/// A process number bound to its kernel start ticks and, when known, its boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// OS process id.
    pub pid: i64,
    /// `/proc/<pid>/stat` starttime as recorded, when known.
    pub started_at: Option<String>,
    /// Boot UUID the process was recorded under, when known.
    pub boot_id: Option<String>,
}

impl ProcessIdentity {
    /// Build an identity from its recorded parts.
    pub fn new(pid: i64, started_at: Option<String>, boot_id: Option<String>) -> Self {
        Self {
            pid,
            started_at,
            boot_id,
        }
    }
}

/// The durable ownership of one turn's harness tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRecord {
    /// Identity of the runner process owning the turn.
    pub runner: ProcessIdentity,
    /// Identity of the harness leader process.
    pub leader: ProcessIdentity,
    /// Leader's process-group/session id, when recorded.
    pub process_group: Option<i64>,
    /// 32-hex turn generation, when recorded.
    pub generation: Option<String>,
    /// How the tree is contained.
    pub containment: Containment,
    /// Per-turn cgroup directory, for [`Containment::Cgroup`] only.
    pub cgroup: Option<PathBuf>,
}

impl HarnessRecord {
    /// True for a record written before boot identities and generations existed.
    pub fn legacy(&self) -> bool {
        self.containment == Containment::Legacy
    }

    /// Serialize, keeping the keys older readers understand.
    ///
    /// Serialize this record to its document form.
    pub fn document(&self, recorded_at: &str) -> serde_json::Value {
        let mut value = serde_json::json!({
            "schema": HARNESS_SCHEMA,
            "runner_pid": self.runner.pid,
            "runner_started_at": self.runner.started_at,
            "runner_boot_id": self.runner.boot_id,
            "pid": self.leader.pid,
            "started_at": self.leader.started_at,
            "boot_id": self.leader.boot_id,
            "process_group": self.process_group,
            "generation": self.generation,
            "containment": self.containment.as_str(),
            "recorded_at": recorded_at,
        });
        if let Some(cgroup) = &self.cgroup {
            value["cgroup"] = serde_json::Value::String(cgroup.to_string_lossy().into_owned());
        }
        value
    }
}

/// True for a canonical lower-case boot UUID.
pub fn valid_boot_id(value: &str) -> bool {
    // `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let lens = [8usize, 4, 4, 4, 12];
    parts.iter().zip(lens.iter()).all(|(part, &len)| {
        part.len() == len
            && part
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// True for a 32-char lowercase hex generation token.
pub fn valid_generation(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// True for canonical start-ticks text: `0` or a 1–20 digit number without leading zeros.
pub fn valid_ticks(value: &str) -> bool {
    // `(?:0|[1-9][0-9]{0,19})`
    if value.is_empty() || value.len() > 20 {
        return false;
    }
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    value.len() == 1 || !value.starts_with('0')
}

/// The procfs root the kernel-evidence readers use by default.
const PROC: &str = "/proc";
/// A boot identity file longer than this is malformed evidence, as in Python.
const MAX_BOOT_ID_BYTES: usize = 128;

/// Return the canonical Linux boot UUID, refusing unreadable evidence.
pub fn current_boot_id() -> Result<String, ForeignOwnershipError> {
    current_boot_id_at(Path::new(PROC))
}

/// [`current_boot_id`] against an explicit procfs root (for tests).
pub fn current_boot_id_at(proc_root: &Path) -> Result<String, ForeignOwnershipError> {
    let path = proc_root.join("sys/kernel/random/boot_id");
    // Read at most one byte past the limit: anything longer is malformed.
    let mut raw = Vec::new();
    let read = std::fs::File::open(&path).and_then(|file| {
        file.take((MAX_BOOT_ID_BYTES + 1) as u64)
            .read_to_end(&mut raw)
    });
    if let Err(error) = read {
        return Err(ForeignOwnershipError::runner_identity_unknown(format!(
            "cannot read the Linux boot identity: {error}"
        )));
    }
    let value = String::from_utf8_lossy(&raw).trim().to_owned();
    if raw.is_empty() || raw.len() > MAX_BOOT_ID_BYTES || !valid_boot_id(&value) {
        return Err(ForeignOwnershipError::runner_identity_unknown(
            "the Linux boot identity is malformed".to_owned(),
        ));
    }
    Ok(value)
}

/// Return one process's identity fields, or `None` only for positive
/// disappearance. Unreadable or malformed procfs evidence is not a death
/// certificate and must not authorize later process signalling.
pub fn read_stat(pid: i64) -> Result<Option<ProcessStat>, ForeignOwnershipError> {
    read_stat_at(Path::new(PROC), pid)
}

/// [`read_stat`] against an explicit procfs root (for tests).
pub fn read_stat_at(
    proc_root: &Path,
    pid: i64,
) -> Result<Option<ProcessStat>, ForeignOwnershipError> {
    let raw = match std::fs::read(proc_root.join(pid.to_string()).join("stat")) {
        Ok(raw) => raw,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ESRCH) =>
        {
            return Ok(None);
        }
        Err(error) => {
            return Err(ForeignOwnershipError::runner_identity_unknown(format!(
                "cannot read runner process {pid} identity: {error}"
            )));
        }
    };
    match parse_process_stat(&raw) {
        Some(parsed) if parsed.pid == pid => Ok(Some(parsed)),
        _ => Err(ForeignOwnershipError::runner_identity_unknown(format!(
            "cannot parse runner process {pid} identity"
        ))),
    }
}

fn quiet_stat_at(proc_root: &Path, pid: i64) -> Option<ProcessStat> {
    read_stat_at(proc_root, pid).ok().flatten()
}

/// List every process procfs shows; vanished or unreadable ones are skipped.
pub fn scan_processes() -> Result<Vec<ProcessStat>, ForeignOwnershipError> {
    scan_processes_at(Path::new(PROC))
}

/// [`scan_processes`] against an explicit procfs root (for tests).
pub fn scan_processes_at(proc_root: &Path) -> Result<Vec<ProcessStat>, ForeignOwnershipError> {
    let entries = std::fs::read_dir(proc_root).map_err(|error| {
        ForeignOwnershipError::runner_identity_unknown(format!(
            "cannot list {}: {error}",
            proc_root.display()
        ))
    })?;
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<i64>() else {
            continue;
        };
        if let Some(parsed) = quiet_stat_at(proc_root, pid) {
            found.push(parsed);
        }
    }
    Ok(found)
}

fn process_number(value: Option<&serde_json::Value>) -> Option<i64> {
    // A boolean is not accepted as a process number; range `1..=2^31-1`.
    let n = value?.as_i64()?;
    if (1..=MAX_PROCESS_ID).contains(&n) {
        Some(n)
    } else {
        None
    }
}

/// Parse `active-harness.json`; malformed ownership errors with
/// `harness_identity_unknown`.
///
/// Parse and validate a harness record document.
pub fn parse_harness_record(
    value: &serde_json::Value,
) -> Result<HarnessRecord, ForeignOwnershipError> {
    let bad = ForeignOwnershipError::harness_identity_unknown;
    let obj = value.as_object().ok_or_else(|| bad("not an object"))?;

    let pid = process_number(obj.get("pid"));
    let started_at = obj.get("started_at").and_then(serde_json::Value::as_str);
    let (pid, started_at) = match (pid, started_at) {
        (Some(pid), Some(started_at)) if pid > 1 => (pid, started_at.to_owned()),
        _ => return Err(bad("no valid harness pid and start time")),
    };

    let runner_pid = process_number(obj.get("runner_pid"));
    let runner_started_raw = obj.get("runner_started_at");
    let runner_started_ok = match runner_started_raw {
        None | Some(serde_json::Value::Null) => true,
        Some(v) => v.as_str().is_some(),
    };
    let runner_pid = match runner_pid {
        Some(p) if runner_started_ok => p,
        _ => return Err(bad("no valid runner pid")),
    };
    let runner_started_at: Option<String> = match runner_started_raw {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => v.as_str().map(str::to_owned),
    };

    if !obj.contains_key("schema") {
        return Ok(HarnessRecord {
            runner: ProcessIdentity::new(runner_pid, runner_started_at, None),
            leader: ProcessIdentity::new(pid, Some(started_at), None),
            process_group: Some(pid),
            generation: None,
            containment: Containment::Legacy,
            cgroup: None,
        });
    }

    if obj.get("schema").and_then(serde_json::Value::as_str) != Some(HARNESS_SCHEMA) {
        let schema = obj
            .get("schema")
            .map(std::string::ToString::to_string)
            .unwrap_or_default();
        return Err(bad(&format!("unsupported schema {schema:?}")));
    }

    let boot_id = obj.get("boot_id").and_then(serde_json::Value::as_str);
    let runner_boot_id = obj
        .get("runner_boot_id")
        .and_then(serde_json::Value::as_str);
    let runner_boot_ok = match obj.get("runner_boot_id") {
        None | Some(serde_json::Value::Null) => true,
        Some(_) => runner_boot_id.is_some_and(valid_boot_id),
    };
    if !boot_id.is_some_and(valid_boot_id) || !runner_boot_ok {
        return Err(bad("no valid boot identity"));
    }
    let boot_id = boot_id.expect("validated above").to_owned();

    let generation = obj.get("generation").and_then(serde_json::Value::as_str);
    if !generation.is_some_and(valid_generation) {
        return Err(bad("no valid generation"));
    }
    let generation = generation.expect("validated above").to_owned();

    let group = process_number(obj.get("process_group"));
    let containment = match obj.get("containment").and_then(serde_json::Value::as_str) {
        Some("process") => Containment::Process,
        Some("cgroup") => Containment::Cgroup,
        _ => return Err(bad("no valid process group or containment")),
    };
    let group = match group {
        Some(g) => g,
        None => return Err(bad("no valid process group or containment")),
    };

    let raw_cgroup = obj.get("cgroup");
    let cgroup: Option<PathBuf> = match containment {
        Containment::Cgroup => {
            let raw = raw_cgroup
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| bad("cgroup containment without a cgroup path"))?;
            let path = PathBuf::from(raw);
            let has_parent = path.components().any(|c| matches!(c, Component::ParentDir));
            let expected_name = format!("{CGROUP_PREFIX}{generation}");
            let name_ok = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy() == expected_name);
            if !path.is_absolute() || has_parent || !name_ok {
                return Err(bad("cgroup path does not name this generation's directory"));
            }
            Some(path)
        }
        _ => {
            if raw_cgroup.is_some_and(|v| !v.is_null()) {
                return Err(bad("process containment with a cgroup path"));
            }
            None
        }
    };

    Ok(HarnessRecord {
        runner: ProcessIdentity::new(
            runner_pid,
            runner_started_at,
            runner_boot_id.map(str::to_owned),
        ),
        leader: ProcessIdentity::new(pid, Some(started_at), Some(boot_id)),
        process_group: Some(group),
        generation: Some(generation),
        containment,
        cgroup,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BOOT: &str = "123e4567-e89b-42d3-a456-426614174000";
    const BOOT2: &str = "223e4567-e89b-42d3-a456-426614174000";
    const GEN: &str = "0123456789abcdef0123456789abcdef";

    fn v1(extra: serde_json::Value) -> serde_json::Value {
        let mut base = json!({
            "schema": HARNESS_SCHEMA,
            "runner_pid": 100,
            "runner_started_at": "111",
            "runner_boot_id": BOOT2,
            "pid": 200,
            "started_at": "222",
            "boot_id": BOOT,
            "process_group": 200,
            "generation": GEN,
            "containment": "process",
            "recorded_at": "2026-11-07T00:00:00Z",
        });
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    }

    #[test]
    fn valid_boot_id_accepts_canonical_uuid() {
        assert!(valid_boot_id(BOOT));
        assert!(!valid_boot_id("123E4567-e89b-42d3-a456-426614174000"));
        assert!(!valid_boot_id("123e4567e89b42d3a456426614174000"));
        assert!(!valid_boot_id("123e4567-e89b-42d3-a456-42661417400"));
        assert!(!valid_boot_id(""));
    }

    #[test]
    fn valid_generation_and_ticks() {
        assert!(valid_generation(GEN));
        assert!(!valid_generation("0123456789ABCDEF0123456789abcdef"));
        assert!(!valid_generation("abc"));
        assert!(valid_ticks("0"));
        assert!(valid_ticks("12345678901234567890"));
        assert!(!valid_ticks("01"));
        assert!(!valid_ticks(""));
        assert!(!valid_ticks("123456789012345678901"));
        assert!(!valid_ticks("-1"));
    }

    #[test]
    fn parses_legacy_record_without_schema() {
        let rec =
            parse_harness_record(&json!({"pid": 200, "started_at": "222", "runner_pid": 100}))
                .unwrap();
        assert!(rec.legacy());
        assert_eq!(rec.leader.pid, 200);
        assert_eq!(rec.runner.pid, 100);
        assert_eq!(rec.process_group, Some(200));
        assert_eq!(rec.generation, None);
    }

    #[test]
    fn parses_v1_process_record() {
        let rec = parse_harness_record(&v1(json!({}))).unwrap();
        assert!(!rec.legacy());
        assert_eq!(rec.containment, Containment::Process);
        assert_eq!(rec.leader.boot_id.as_deref(), Some(BOOT));
        assert_eq!(rec.runner.boot_id.as_deref(), Some(BOOT2));
        assert_eq!(rec.generation.as_deref(), Some(GEN));
    }

    #[test]
    fn parses_v1_cgroup_record() {
        let path = format!("/sys/fs/cgroup/agentctl-harness-{GEN}");
        let rec =
            parse_harness_record(&v1(json!({"containment": "cgroup", "cgroup": path}))).unwrap();
        assert_eq!(rec.containment, Containment::Cgroup);
        assert_eq!(
            rec.cgroup,
            Some(PathBuf::from(format!(
                "/sys/fs/cgroup/agentctl-harness-{GEN}"
            )))
        );
    }

    #[test]
    fn document_round_trips_through_parse() {
        let rec = parse_harness_record(&v1(json!({}))).unwrap();
        let doc = rec.document("2026-11-07T00:00:00Z");
        assert_eq!(doc["schema"], json!(HARNESS_SCHEMA));
        assert_eq!(doc["containment"], json!("process"));
        let again = parse_harness_record(&doc).unwrap();
        assert_eq!(again, rec);
    }

    #[test]
    fn rejects_non_object() {
        let err = parse_harness_record(&json!([1, 2, 3])).unwrap_err();
        assert_eq!(err.code, "harness_identity_unknown");
        assert!(err.message.contains("not an object"));
    }

    #[test]
    fn rejects_bad_pid_and_started_at() {
        for v in [
            json!({"pid": 1, "started_at": "222", "runner_pid": 100}),
            json!({"pid": 200, "runner_pid": 100}),
            json!({"pid": "200", "started_at": "222", "runner_pid": 100}),
            json!({"pid": true, "started_at": "222", "runner_pid": 100}),
            json!({"pid": 200, "started_at": "222"}),
        ] {
            assert!(parse_harness_record(&v).is_err(), "expected rejection: {v}");
        }
    }

    #[test]
    fn rejects_unsupported_schema() {
        let err = parse_harness_record(&v1(json!({"schema": "other/v9"}))).unwrap_err();
        assert!(err.message.contains("unsupported schema"));
    }

    #[test]
    fn rejects_bad_boot_generation_group_containment() {
        for extra in [
            json!({"boot_id": "not-a-uuid"}),
            json!({"runner_boot_id": "not-a-uuid"}),
            json!({"generation": "xyz"}),
            json!({"process_group": 0}),
            json!({"containment": "weird"}),
        ] {
            assert!(parse_harness_record(&v1(extra)).is_err());
        }
    }

    #[test]
    fn rejects_cgroup_path_mismatch() {
        for extra in [
            json!({"containment": "cgroup"}),
            json!({"containment": "cgroup", "cgroup": "relative/agentctl-harness-0123456789abcdef0123456789abcdef"}),
            json!({"containment": "cgroup", "cgroup": "/sys/fs/cgroup/agentctl-harness-deadbeef"}),
            json!({"containment": "cgroup", "cgroup": format!("/sys/fs/cgroup/../agentctl-harness-{GEN}")}),
        ] {
            assert!(
                parse_harness_record(&v1(extra.clone())).is_err(),
                "expected rejection: {extra:?}"
            );
        }
    }

    #[test]
    fn rejects_process_containment_with_cgroup() {
        assert!(parse_harness_record(&v1(json!({"cgroup": "/sys/fs/cgroup/x"}))).is_err());
    }

    fn fake_proc(test: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("agentctl-b3-2-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys/kernel/random")).unwrap();
        root
    }

    fn stat_record(pid: i64, starttime: u64) -> String {
        let mut tail: Vec<String> = vec![
            "S".to_owned(),
            "10".to_owned(),
            "20".to_owned(),
            "20".to_owned(),
        ];
        tail.extend(std::iter::repeat_n("0".to_owned(), 15));
        tail.push(starttime.to_string());
        tail.extend(std::iter::repeat_n("0".to_owned(), 30));
        format!("{pid} (fake proc) {}\n", tail.join(" "))
    }

    fn write_stat(root: &Path, dir_pid: i64, record: &str) {
        let dir = root.join(dir_pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stat"), record).unwrap();
    }

    #[test]
    fn current_boot_id_reads_only_canonical_evidence() {
        let root = fake_proc("boot");
        let boot_file = root.join("sys/kernel/random/boot_id");
        std::fs::write(&boot_file, format!("{BOOT}\n")).unwrap();
        assert_eq!(current_boot_id_at(&root).unwrap(), BOOT);

        std::fs::write(&boot_file, "not-a-uuid\n").unwrap();
        let err = current_boot_id_at(&root).unwrap_err();
        assert_eq!(err.code, "runner_identity_unknown");
        assert!(err.message.contains("malformed"), "{}", err.message);

        // Over the byte limit is malformed even when the text would parse.
        std::fs::write(&boot_file, format!("{BOOT}{}\n", " ".repeat(200))).unwrap();
        assert!(current_boot_id_at(&root).is_err());

        std::fs::remove_file(&boot_file).unwrap();
        let err = current_boot_id_at(&root).unwrap_err();
        assert!(err.message.contains("cannot read"), "{}", err.message);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_stat_distinguishes_gone_from_malformed() {
        let root = fake_proc("stat");
        write_stat(&root, 4242, &stat_record(4242, 987654));
        let stat = read_stat_at(&root, 4242).unwrap().unwrap();
        assert_eq!(stat.pid, 4242);
        assert_eq!(stat.starttime, 987654);

        // Positive disappearance is the only `None`.
        assert_eq!(read_stat_at(&root, 7777).unwrap(), None);

        write_stat(&root, 5555, "garbage");
        let err = read_stat_at(&root, 5555).unwrap_err();
        assert_eq!(err.code, "runner_identity_unknown");
        assert!(err.message.contains("cannot parse"), "{}", err.message);

        // A record naming a different pid is malformed evidence, not `None`.
        write_stat(&root, 6666, &stat_record(9999, 1));
        assert!(read_stat_at(&root, 6666).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_processes_skips_the_unreadable() {
        let root = fake_proc("scan");
        write_stat(&root, 4242, &stat_record(4242, 987654));
        write_stat(&root, 99, "garbage");
        std::fs::create_dir_all(root.join("55")).unwrap();
        std::fs::create_dir_all(root.join("self")).unwrap();
        let found = scan_processes_at(&root).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pid, 4242);

        let err = scan_processes_at(&root.join("missing")).unwrap_err();
        assert!(err.message.contains("cannot list"), "{}", err.message);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_procfs_evidence_names_this_process() {
        let boot = current_boot_id().unwrap();
        assert!(valid_boot_id(&boot));
        let own = i64::from(std::process::id());
        let stat = read_stat(own).unwrap().unwrap();
        assert_eq!(stat.pid, own);
        assert!(scan_processes().unwrap().iter().any(|s| s.pid == own));
    }
}
