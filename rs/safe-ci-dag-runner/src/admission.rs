//! Runner-enforced resource-exclusivity admission: the `solo_validate` capability.
//!
//! A `validate` invocation demands SOLO possession of the box: it is REFUSED ADMISSION while
//! another validate OR a benchmark harness holds the box. Possession is advertised as a small
//! holder file under a shared holders directory; liveness is proven PER-HOLDER (`/proc/<pid>`
//! exists), so a stale holder left by a crashed process is ignored and best-effort unlinked. The
//! predicate therefore binds refusal to a *live* competing holder rather than to the mere presence
//! of a file (Proxy-Binding: carry the condition — the live pid — with the value).
//!
//! The holder file format is two lines (`role=<role>` / `pid=<pid>`) so the Rust and Python engines
//! read each other's holders. MUST stay behaviorally identical to
//! `py/safe_ci_dag_runner/admission.py`.

use std::fs;
use std::path::{Path, PathBuf};

/// The solo exclusivity role.
pub const VALIDATE: &str = "validate";
/// A benchmark harness holding the box.
pub const BENCHMARK: &str = "benchmark";

/// A live process currently possessing the box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub role: String,
    pub pid: i64,
    pub path: PathBuf,
}

/// True iff `pid` names a live process (`/proc/<pid>` exists), matching the Python engine.
fn pid_alive(pid: i64) -> bool {
    pid > 0 && Path::new(&format!("/proc/{pid}")).exists()
}

/// Live holders in `holders_dir`, sorted by `(role, pid)`.
///
/// A holder file whose pid is dead is SKIPPED (crash-safe) and best-effort unlinked. `exclude_pid`
/// drops the caller's own holder so a runner never refuses itself.
pub fn scan_live_holders(holders_dir: &Path, exclude_pid: Option<i64>) -> Vec<Holder> {
    let mut holders: Vec<Holder> = Vec::new();
    let mut entries: Vec<PathBuf> = match fs::read_dir(holders_dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => return holders,
    };
    entries.sort();
    for entry in entries {
        if entry.extension().and_then(|s| s.to_str()) != Some("holder") {
            continue;
        }
        let text = match fs::read_to_string(&entry) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut role: Option<String> = None;
        let mut pid: Option<i64> = None;
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("role=") {
                role = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("pid=") {
                pid = v.trim().parse::<i64>().ok();
            }
        }
        let (role, pid) = match (role, pid) {
            (Some(r), Some(p)) => (r, p),
            _ => continue,
        };
        if !pid_alive(pid) {
            let _ = fs::remove_file(&entry);
            continue;
        }
        if exclude_pid == Some(pid) {
            continue;
        }
        holders.push(Holder {
            role,
            pid,
            path: entry,
        });
    }
    holders.sort_by(|a, b| (a.role.as_str(), a.pid).cmp(&(b.role.as_str(), b.pid)));
    holders
}

/// The exclusivity predicate. `Some(reason)` when `role` must be REFUSED given the live `holders`,
/// else `None` (admit).
///
/// * a `validate` node is SOLO: refused while ANY live foreign holder (validate or benchmark) holds
///   the box;
/// * a `benchmark` node is refused while a live `validate` holder holds the box (validate's solo
///   claim wins);
/// * any other role is unconstrained (admit).
pub fn solo_validate_refusal(role: &str, holders: &[Holder]) -> Option<String> {
    let blockers: Vec<&Holder> = match role {
        VALIDATE => holders.iter().collect(),
        BENCHMARK => holders.iter().filter(|h| h.role == VALIDATE).collect(),
        _ => return None,
    };
    if blockers.is_empty() {
        return None;
    }
    let who = blockers
        .iter()
        .map(|h| format!("{}(pid {})", h.role, h.pid))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("{role} refused admission: box held by {who}"))
}

/// Write this process's holder file and return its path. The caller MUST [`release`] it.
pub fn acquire(holders_dir: &Path, role: &str, pid: i64) -> std::io::Result<PathBuf> {
    fs::create_dir_all(holders_dir)?;
    let path = holders_dir.join(format!("{role}.{pid}.holder"));
    fs::write(&path, format!("role={role}\npid={pid}\n"))?;
    Ok(path)
}

/// Best-effort remove a holder file created by [`acquire`].
pub fn release(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(role: &str, pid: i64) -> Holder {
        Holder {
            role: role.into(),
            pid,
            path: PathBuf::from("x"),
        }
    }

    #[test]
    fn validate_is_refused_by_any_live_holder() {
        // NEGATIVE: a validate node is refused while a benchmark OR another validate holds the box.
        assert!(solo_validate_refusal(VALIDATE, &[holder(BENCHMARK, 1)]).is_some());
        assert!(solo_validate_refusal(VALIDATE, &[holder(VALIDATE, 2)]).is_some());
    }

    #[test]
    fn validate_is_admitted_when_box_is_free() {
        // POSITIVE: no live holders -> admit.
        assert!(solo_validate_refusal(VALIDATE, &[]).is_none());
    }

    #[test]
    fn benchmark_yields_only_to_validate() {
        assert!(solo_validate_refusal(BENCHMARK, &[holder(VALIDATE, 1)]).is_some());
        // A benchmark does not exclude another benchmark.
        assert!(solo_validate_refusal(BENCHMARK, &[holder(BENCHMARK, 1)]).is_none());
    }

    #[test]
    fn scan_skips_dead_and_self_and_reads_live() {
        let dir = std::env::temp_dir().join(format!("scdr-adm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // Dead pid -> skipped and unlinked; self pid -> excluded; a live foreign pid (pid 1, always
        // present on Linux) -> reported.
        let dead = acquire(&dir, BENCHMARK, 999_999_999).unwrap();
        let me = acquire(&dir, VALIDATE, std::process::id() as i64).unwrap();
        let _live = acquire(&dir, BENCHMARK, 1).unwrap();
        let live = scan_live_holders(&dir, Some(std::process::id() as i64));
        assert_eq!(live.len(), 1, "only the live foreign holder is reported");
        assert_eq!(live[0].pid, 1);
        assert!(!dead.exists(), "dead holder is unlinked");
        release(&me);
        let _ = fs::remove_dir_all(&dir);
    }
}
