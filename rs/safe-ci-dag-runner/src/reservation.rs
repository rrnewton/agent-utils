//! Durable, collision-free CPU-core reservations shared by local processes.

// Durable, cross-process CPU-core reservations.
//
// The ledger is shared with the Python distribution: both implementations lock the same sibling
// `.lock` file, read/write the same JSON schema, fingerprint holders by `(pid, /proc starttime)`,
// reclaim dead holders, and select only from free allowed CPUs.  A reservation is released on
// `Drop`; a crash is recovered by the next ledger operation.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Number, Value};

use crate::cgroup::pick_least_busy_free_cores_excluding;

/// Environment variable overriding the host-wide reservation ledger.
pub const LEDGER_ENV: &str = "SAFE_CI_CORE_LEDGER";

#[derive(Clone, Debug, PartialEq)]
struct Record {
    pid: u32,
    starttime: Option<u64>,
    cores: Vec<usize>,
    tag: String,
    ts: f64,
}

impl Record {
    fn from_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let pid = value_u64(obj.get("pid")?)? as u32;
        let starttime = match obj.get("starttime") {
            Some(Value::Null) | None => None,
            Some(v) => value_u64(v),
        };
        let cores = obj
            .get("cores")?
            .as_array()?
            .iter()
            .filter_map(|v| value_u64(v).map(|n| n as usize))
            .collect();
        let tag = value_text(obj.get("tag")).unwrap_or_default();
        let ts = match obj.get("ts") {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(Value::String(s)) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        };
        Some(Self {
            pid,
            starttime,
            cores,
            tag,
            ts,
        })
    }

    fn to_value(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("pid".into(), Value::from(self.pid));
        obj.insert(
            "starttime".into(),
            self.starttime.map(Value::from).unwrap_or(Value::Null),
        );
        obj.insert(
            "cores".into(),
            Value::Array(self.cores.iter().copied().map(Value::from).collect()),
        );
        obj.insert("tag".into(), Value::String(self.tag.clone()));
        obj.insert(
            "ts".into(),
            Number::from_f64(self.ts)
                .map(Value::Number)
                .unwrap_or_else(|| Value::from(0)),
        );
        Value::Object(obj)
    }
}

fn value_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn value_text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(v)) => Some(v.to_string()),
        _ => None,
    }
}

/// Resolve the host-wide reservation-ledger path from the environment or runtime directory.
pub fn default_ledger_path() -> PathBuf {
    if let Some(path) = std::env::var_os(LEDGER_ENV) {
        return PathBuf::from(path);
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let runtime = PathBuf::from(runtime);
        if runtime.is_dir() {
            return runtime
                .join("safe-ci-dag-runner")
                .join("core-reservations.json");
        }
    }
    // SAFETY: geteuid has no preconditions and does not access memory.
    let uid = unsafe { libc::geteuid() };
    std::env::temp_dir()
        .join(format!("safe-ci-dag-runner-{uid}"))
        .join("core-reservations.json")
}

fn proc_starttime(pid: u32) -> Option<u64> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rparen = text.rfind(')')?;
    text.get(rparen + 2..)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn holder_alive(record: &Record) -> bool {
    match proc_starttime(record.pid) {
        None => false,
        Some(_) if record.starttime.is_none() => true,
        Some(current) => record.starttime == Some(current),
    }
}

struct LedgerLock {
    file: File,
}

impl LedgerLock {
    fn acquire(ledger: &Path) -> Result<Self, String> {
        let lock_path = PathBuf::from(format!("{}.lock", ledger.display()));
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create ledger directory: {e}"))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| format!("open ledger lock {}: {e}", lock_path.display()))?;
        // SAFETY: flock operates on this live file descriptor and does not retain a pointer.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(format!(
                "lock reservation ledger {}: {}",
                lock_path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { file })
    }
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        // SAFETY: the descriptor remains valid until `file` is dropped after this method.
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn load(path: &Path) -> Vec<Record> {
    let mut text = String::new();
    if File::open(path)
        .and_then(|mut f| f.read_to_string(&mut text))
        .is_err()
    {
        return Vec::new();
    }
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    root.get("reservations")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Record::from_value).collect())
        .unwrap_or_default()
}

fn store(path: &Path, records: &[Record]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("create ledger directory: {e}"))?;
    let payload = Value::Object(Map::from_iter([(
        "reservations".to_string(),
        Value::Array(records.iter().map(Record::to_value).collect()),
    )]));
    let bytes = serde_json::to_vec(&payload).map_err(|e| format!("encode ledger: {e}"))?;
    let mut created: Option<(PathBuf, File)> = None;
    for attempt in 0..100u32 {
        let temp = parent.join(format!(
            ".core-reservations.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => {
                created = Some((temp, file));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("create temporary ledger: {e}")),
        }
    }
    let (temp, mut file) =
        created.ok_or_else(|| "could not allocate temporary ledger".to_string())?;
    let result = (|| {
        file.write_all(&bytes)
            .map_err(|e| format!("write temporary ledger: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("sync temporary ledger: {e}"))?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| format!("replace reservation ledger: {e}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn sweep(records: Vec<Record>) -> (Vec<Record>, Vec<Record>) {
    records.into_iter().partition(holder_alive)
}

/// A collision-free core reservation.  Dropping it releases its ledger record.
#[derive(Debug)]
pub struct Reservation {
    /// Exact reserved CPU IDs.
    pub cores: Vec<usize>,
    pid: u32,
    starttime: Option<u64>,
    tag: String,
    ledger: PathBuf,
    released: bool,
}

impl Reservation {
    /// Release this reservation.  Idempotent.
    pub fn release(&mut self) -> Result<(), String> {
        if self.released {
            return Ok(());
        }
        let _lock = LedgerLock::acquire(&self.ledger)?;
        let kept: Vec<Record> = load(&self.ledger)
            .into_iter()
            .filter(|r| {
                !(r.pid == self.pid
                    && r.starttime == self.starttime
                    && r.tag == self.tag
                    && r.cores == self.cores)
            })
            .collect();
        store(&self.ledger, &kept)?;
        self.released = true;
        Ok(())
    }

    /// Path of the ledger holding this reservation.
    pub fn ledger(&self) -> &Path {
        &self.ledger
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Reserve `k` disjoint, least-busy cores for this process.
pub fn acquire(
    k: i64,
    tag: &str,
    sample_s: f64,
    ledger: Option<&Path>,
    exclude: &HashSet<usize>,
    max_irq_rate: Option<f64>,
) -> Result<Reservation, String> {
    if k < 1 {
        return Err(format!("k must be >= 1, got {k}"));
    }
    if !sample_s.is_finite() || sample_s < 0.0 {
        return Err(format!("sample_s must be finite and >= 0, got {sample_s}"));
    }
    let path = ledger
        .map(Path::to_path_buf)
        .unwrap_or_else(default_ledger_path);
    let pid = std::process::id();
    let starttime = proc_starttime(pid);
    let _lock = LedgerLock::acquire(&path)?;
    let (live, _) = sweep(load(&path));
    let mut held = exclude.clone();
    for record in &live {
        held.extend(record.cores.iter().copied());
    }
    let cores = pick_least_busy_free_cores_excluding(k, sample_s, &held, max_irq_rate);
    if cores.len() < k as usize {
        store(&path, &live)?;
        return Err(format!(
            "requested {k} core(s) but only {} free-and-unheld (held by {} live reservation(s): {:?}; allowed set may also be smaller)",
            cores.len(),
            live.len(),
            {
                let mut values: Vec<usize> = held.into_iter().collect();
                values.sort_unstable();
                values
            }
        ));
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let mut updated = live;
    updated.push(Record {
        pid,
        starttime,
        cores: cores.clone(),
        tag: tag.to_string(),
        ts,
    });
    store(&path, &updated)?;
    Ok(Reservation {
        cores,
        pid,
        starttime,
        tag: tag.to_string(),
        ledger: path,
        released: false,
    })
}

/// Reclaim and return dead-holder records.
pub fn reclaim_dead(ledger: Option<&Path>) -> Result<Vec<Value>, String> {
    let path = ledger
        .map(Path::to_path_buf)
        .unwrap_or_else(default_ledger_path);
    let _lock = LedgerLock::acquire(&path)?;
    let (live, dead) = sweep(load(&path));
    if !dead.is_empty() {
        store(&path, &live)?;
    }
    Ok(dead.iter().map(Record::to_value).collect())
}

/// Return all currently held cores after reclaiming dead holders.
pub fn held_cores(ledger: Option<&Path>) -> Result<Vec<usize>, String> {
    let path = ledger
        .map(Path::to_path_buf)
        .unwrap_or_else(default_ledger_path);
    let _lock = LedgerLock::acquire(&path)?;
    let (live, dead) = sweep(load(&path));
    if !dead.is_empty() {
        store(&path, &live)?;
    }
    let mut cores: Vec<usize> = live
        .iter()
        .flat_map(|record| record.cores.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    cores.sort_unstable();
    Ok(cores)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_ledger(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "safe-ci-rs-reservation-{}-{name}.json",
            std::process::id()
        ))
    }

    #[test]
    fn acquire_release_and_shared_schema() {
        let path = temp_ledger("release");
        let _ = fs::remove_file(&path);
        let mut reservation =
            acquire(1, "unit", 0.001, Some(&path), &HashSet::new(), None).unwrap();
        assert_eq!(held_cores(Some(&path)).unwrap(), reservation.cores);
        let parsed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(parsed["reservations"][0]["starttime"].is_number());
        reservation.release().unwrap();
        assert!(held_cores(Some(&path)).unwrap().is_empty());
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.lock", path.display()));
    }

    #[test]
    fn acquire_rejects_invalid_sample_windows() {
        let path = temp_ledger("sample-window");
        for sample_s in [-1.0, f64::NAN, f64::INFINITY] {
            let error = acquire(1, "unit", sample_s, Some(&path), &HashSet::new(), None)
                .expect_err("invalid sample window must be rejected");
            assert!(error.contains("sample_s must be finite and >= 0"));
        }
    }

    #[test]
    fn dead_holder_is_reclaimed() {
        let path = temp_ledger("dead");
        let dead = Record {
            pid: u32::MAX,
            starttime: Some(1),
            cores: vec![0],
            tag: "dead".into(),
            ts: 0.0,
        };
        store(&path, &[dead]).unwrap();
        let reclaimed = reclaim_dead(Some(&path)).unwrap();
        assert_eq!(reclaimed.len(), 1);
        assert!(held_cores(Some(&path)).unwrap().is_empty());
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.lock", path.display()));
    }

    #[test]
    fn releasing_one_same_tag_reservation_keeps_the_other() {
        if crate::cgroup::pick_least_busy_free_cores(2, 0.0).len() < 2 {
            return;
        }
        let path = temp_ledger("same-tag");
        let _ = fs::remove_file(&path);
        let mut first = acquire(1, "same", 0.001, Some(&path), &HashSet::new(), None).unwrap();
        let mut second = acquire(1, "same", 0.001, Some(&path), &HashSet::new(), None).unwrap();
        assert_ne!(first.cores, second.cores);
        first.release().unwrap();
        assert_eq!(held_cores(Some(&path)).unwrap(), second.cores);
        second.release().unwrap();
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.lock", path.display()));
    }
}
