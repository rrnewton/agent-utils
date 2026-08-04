//! Two-level Linux cgroup-v2 containment for the DAG runner (Rust port of `cgroup.py`).
//!
//! The runner re-execs itself inside a transient `systemd-run --user --scope` (a DELEGATED
//! outer cgroup), then carves one CHILD cgroup per step under it. Each step's bash leader
//! self-moves into its child cgroup BEFORE forking any grandchild, so a per-step
//! `cgroup.kill` SIGKILLs the WHOLE subtree atomically — including `setsid` / double-fork
//! escapees a process-group kill misses. Per-step `memory.max` (+ `memory.swap.max=0`) makes a
//! single runaway step OOM-killed at its own cap while the rest of the run and the host survive.
//!
//! GRACEFUL DEGRADATION + NO SILENT FAILURE: everything is best-effort, but a best-effort
//! cgroupfs write that would drop a requested cap never fails silently — it emits a visible
//! `warn()` on stderr. When cgroup-v2 + a systemd `--user` scope are unavailable the manager
//! reports `enabled() == false` and the caller falls back to process-group teardown.
//!
//! This mirrors the OBSERVABLE behavior of the Python `cgroup.py`; the systemd/cgroupfs
//! interactions (env sentinels, slice/unit names, the supervisor drain) are kept identical.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::thread;

use crate::sizing::cpu_count;

/// cgroup-v2 unified hierarchy mount point (Linux-only, matching the target).
const CGROUP_ROOT: &str = "/sys/fs/cgroup";
/// Shared parent slice for ALL concurrent runs (one aggregate CPUQuota bounds their sum).
const SLICE_NAME: &str = "safe-ci.slice";
/// Prefix for the per-run transient scope unit (`<prefix>-<pid>`).
const UNIT_PREFIX: &str = "safe-ci";
/// Environment sentinel set in the re-exec'd (in-scope) child.
const ENV_IN_SCOPE: &str = "SAFE_CI_IN_SCOPE";
/// Env var carrying the outer scope unit name to the in-scope child.
const ENV_SCOPE_UNIT: &str = "SAFE_CI_SCOPE_UNIT";
/// Child cgroup the runner vacates into (cgroup-v2 "no internal processes" rule).
const SUPERVISOR: &str = "supervisor";
/// Per-step child cgroup directory prefix (also the normal-exit backstop scan key).
const STEP_PREFIX: &str = "step-";
/// Prefix for every log/warning line this module prints.
const LOG_PREFIX: &str = "[safe-ci]";
/// Fraction of WHOLE-SYSTEM CPU the shared aggregate slice may use (leaves ~10% headroom).
const DEFAULT_CPU_BUDGET_FRACTION: f64 = 0.90;

/// Per-step containment: create, cap, measure, and tear down a child cgroup per step.
///
/// The object-safe Rust analogue of Python's `CgroupManager` protocol; a single manager is
/// shared across the run (behind an `Arc`). Every method is `&self` (state lives on cgroupfs,
/// so no interior mutability is needed).
pub trait CgroupManager: Send + Sync {
    /// Whether per-step containment is actually usable on this host.
    fn enabled(&self) -> bool;
    /// Wrap a step's command so its bash leader joins the step's child cgroup before forking,
    /// applying the inner memory/CPU caps. Returns `cmd` unchanged when disabled.
    fn prepare_command(
        &self,
        tag: &str,
        cmd: &str,
        mem_max: Option<i64>,
        cpu_count: Option<i64>,
    ) -> String;
    /// SIGKILL the step's whole cgroup subtree (`cgroup.kill`); `true` if the write landed.
    fn kill(&self, tag: &str) -> bool;
    /// Remove the step's now-empty child cgroup dir (best-effort).
    fn cleanup(&self, tag: &str);
    /// Kernel OOM-kill events inside the step's cgroup (`memory.events` `oom_kill`).
    fn oom_kills(&self, tag: &str) -> i64;
    /// Peak resident memory (bytes) of the step's cgroup (`memory.peak`).
    fn peak_bytes(&self, tag: &str) -> Option<i64>;
    /// Per-step cgroup-v2 CPU counters from `cpu.stat`.
    fn cpu_stats(&self, tag: &str) -> Option<BTreeMap<String, i64>>;
    /// Per-step CPU pressure-stall averages (`cpu.pressure` `some` line: `avg10`, `avg60`),
    /// sampled at step start + end to attribute contention. `None` when disabled/unreadable.
    fn cpu_pressure(&self, tag: &str) -> Option<BTreeMap<String, f64>>;
    /// Current descendant thread count from the step's `cgroup.threads`.
    fn thread_count(&self, tag: &str) -> Option<i64>;
    /// NORMAL-EXIT backstop: `cgroup.kill` + `rmdir` every remaining step child cgroup.
    fn kill_all_remaining(&self) -> i64;
}

/// Emit a visible degraded-enforcement warning (No Silent Failure).
fn warn(msg: &str) {
    eprintln!("{LOG_PREFIX} WARNING: degraded enforcement: {msg}");
}

/// A cgroup directory name for a step tag (cgroup-v2 names may not contain '/').
fn sanitize(tag: &str) -> String {
    let mut s = String::with_capacity(tag.len() + STEP_PREFIX.len());
    s.push_str(STEP_PREFIX);
    for c in tag.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            s.push(c);
        } else {
            s.push('_');
        }
    }
    s
}

/// Filesystem path of THIS process's cgroup v2, via `/proc/self/cgroup` (`0::<path>`).
fn my_cgroup_path() -> Option<PathBuf> {
    let text = fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in text.lines() {
        if let Some(rel) = line.strip_prefix("0::") {
            let rel = rel.trim_start_matches('/');
            return Some(Path::new(CGROUP_ROOT).join(rel));
        }
    }
    None
}

/// Read and trim a cgroup interface file, or `None`.
fn read_trim(group: &Path, name: &str) -> Option<String> {
    fs::read_to_string(group.join(name))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Whole granted cores from a `cpu.max` string (`"<quota> <period>"`), or `None` when
/// unbounded (`"max ..."`) / unparseable. Floors to >=1 for any positive quota. Mirrors
/// Python `_cpu_max_cores`.
fn cpu_max_cores(value: Option<String>) -> Option<i64> {
    let value = value?;
    let mut parts = value.split_whitespace();
    let quota = parts.next()?;
    let period = parts.next()?;
    if parts.next().is_some() || quota == "max" {
        return None;
    }
    let quota: i64 = quota.parse().ok()?;
    let period: i64 = period.parse().ok()?;
    if quota <= 0 || period <= 0 {
        return None;
    }
    Some((quota / period).max(1))
}

/// Byte cap from a `memory.max` string, or `None` when unbounded/unparseable. Mirrors Python
/// `_memory_max_bytes`.
fn memory_max_bytes(value: Option<String>) -> Option<i64> {
    let value = value?;
    if value == "max" {
        return None;
    }
    value.parse().ok()
}

/// Create a cgroup directory, tolerating "already exists".
fn make_dir(path: &Path) -> std::io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

fn in_scope() -> bool {
    std::env::var(ENV_IN_SCOPE).ok().as_deref() == Some("1")
}

/// Whether this process is already running inside the managed cgroup scope (the re-exec set the
/// `SAFE_CI_IN_SCOPE` sentinel). The CLI checks this to decide whether to re-exec.
pub fn is_in_scope() -> bool {
    in_scope()
}

/// systemd `CPUQuota` percentage for `fraction` of ALL cores (min 100%).
fn cpu_quota_percent(fraction: f64) -> i64 {
    let ncpu = cpu_count().max(1);
    ((ncpu as f64 * fraction * 100.0).round() as i64).max(100)
}

/// True iff `systemd-run --user --scope` actually works here (cached).
fn systemd_scope_available() -> bool {
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| {
        Command::new("systemd-run")
            .args([
                "--user",
                "--scope",
                "--quiet",
                &format!("--unit=safe-ci-probe-{}", std::process::id()),
                "true",
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Create/refresh the shared aggregate slice with a `CPUQuota` bounding the SUM of CPU across
/// every scope launched under it. Idempotent; best-effort (returns false when unavailable).
fn ensure_aggregate_slice(fraction: f64) -> bool {
    let quota = cpu_quota_percent(fraction);
    let start = Command::new("systemctl")
        .args(["--user", "start", SLICE_NAME])
        .output();
    let set = Command::new("systemctl")
        .args([
            "--user",
            "--runtime",
            "set-property",
            SLICE_NAME,
            &format!("CPUQuota={quota}%"),
        ])
        .output();
    matches!((start, set), (Ok(a), Ok(b)) if a.status.success() && b.status.success())
}

/// Re-exec this process inside a transient `systemd-run --user --scope` (a delegated cgroup).
///
/// On success `exec` REPLACES this process and never returns. The bool return distinguishes the
/// non-exec paths, matching Python's `reexec_in_scope`:
/// * `true`  — already in-scope (anti-recursion) or intentionally skipped in CI: proceed.
/// * `false` — systemd scope unavailable or the exec failed: the caller must refuse to run
///   advisory-only (No Silent Failure — the reason is on stderr).
pub fn reexec_in_scope(memory_max: Option<i64>, cpu_count: Option<i64>) -> bool {
    if in_scope() {
        return true;
    }
    if std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok() {
        return true;
    }
    if !systemd_scope_available() {
        eprintln!(
            "{LOG_PREFIX} ERROR: systemd --user scope is unavailable; refusing advisory-only \
             containment."
        );
        return false;
    }

    let pid = std::process::id();
    let unit = format!("{UNIT_PREFIX}-{pid}");
    let mut args: Vec<String> = vec![
        "--user".into(),
        "--scope".into(),
        "--collect".into(),
        "--quiet".into(),
        format!("--unit={unit}"),
        "-p".into(),
        "Delegate=yes".into(),
        "-p".into(),
        "MemorySwapMax=0".into(),
    ];
    if let Some(cpu) = cpu_count {
        args.push("-p".into());
        args.push(format!("CPUQuota={}%", cpu * 100));
    }
    if ensure_aggregate_slice(DEFAULT_CPU_BUDGET_FRACTION) {
        args.push(format!("--slice={SLICE_NAME}"));
        eprintln!(
            "{LOG_PREFIX} CPU cap: shared {SLICE_NAME} CPUQuota={}% (~90% of {} cores, AGGREGATE \
             across concurrent runs).",
            cpu_quota_percent(DEFAULT_CPU_BUDGET_FRACTION),
            cpu_count.unwrap_or_else(crate::sizing::cpu_count)
        );
    }
    if let Some(m) = memory_max {
        args.push("-p".into());
        args.push(format!("MemoryMax={m}"));
    }
    // Scope-wide build-job default, derived from the granted cores + memory cap, so a command
    // run directly in the scope (not via a per-step child) still can't compute NUM_JOBS=<all
    // cores>. Per-step prepare_command refines it downward; this is the inherited floor.
    args.push(format!(
        "--setenv=CARGO_BUILD_JOBS={}",
        crate::sizing::derive_build_jobs(cpu_count, memory_max)
    ));
    args.push(format!("--setenv={ENV_IN_SCOPE}=1"));
    args.push(format!("--setenv={ENV_SCOPE_UNIT}={unit}.scope"));
    args.push("--".into());

    match std::env::current_exe() {
        Ok(exe) => args.push(exe.to_string_lossy().into_owned()),
        Err(e) => {
            eprintln!(
                "{LOG_PREFIX} ERROR: cannot resolve own executable ({e}); refusing to run \
                       without cgroup enforcement."
            );
            return false;
        }
    }
    args.extend(std::env::args().skip(1));

    eprintln!(
        "{LOG_PREFIX} re-exec inside transient systemd scope {unit}.scope (two-level cgroup; \
         full-descendant cleanup on exit)…"
    );
    // exec replaces this process on success; it only returns on error.
    let err = Command::new("systemd-run").args(&args).exec();
    eprintln!(
        "{LOG_PREFIX} ERROR: systemd-run exec failed ({err}); refusing to run without cgroup \
         enforcement."
    );
    false
}

/// The OUTER scope's cgroup path, derived from THIS process's own cgroup (no systemctl call).
fn scope_cgroup_from_self() -> Option<PathBuf> {
    let mine = my_cgroup_path()?;
    let name = mine.file_name()?.to_str()?;
    if name == SUPERVISOR {
        return mine.parent().map(Path::to_path_buf);
    }
    if name.ends_with(".scope") {
        return Some(mine);
    }
    None
}

/// Install a SIGINT/SIGTERM handler that tears down the WHOLE outer scope, then exits.
///
/// Killing only the runner would leave `setsid`-escapee orphans alive in the scope cgroup
/// (killpg and systemd `--collect` both miss them). On signal we instead `cgroup.kill` the whole
/// scope subtree and `systemctl --user stop` the unit. No-op when not in-scope. Uses
/// `signal-hook` so the handler is registered without `unsafe` (safe-rust invariant).
pub fn install_scope_teardown() {
    if !in_scope() {
        return;
    }
    let unit = std::env::var(ENV_SCOPE_UNIT).ok();
    let scope_cg = scope_cgroup_from_self();
    if unit.is_none() && scope_cg.is_none() {
        return;
    }
    let mut signals = match signal_hook::iterator::Signals::new([SIGINT_NUM, SIGTERM_NUM]) {
        Ok(s) => s,
        Err(e) => {
            warn(&format!(
                "could not install scope-teardown signal handler ({e})"
            ));
            return;
        }
    };
    thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            eprintln!(
                "{LOG_PREFIX} signal {sig} — stopping outer scope (tears down all steps + orphans)…"
            );
            if let Some(cg) = &scope_cg {
                let _ = fs::write(cg.join("cgroup.kill"), "1");
            }
            if let Some(u) = &unit {
                let _ = Command::new("systemctl")
                    .args(["--user", "stop", u])
                    .output();
            }
            std::process::exit(128 + sig);
        }
    });
}

const SIGINT_NUM: i32 = 2;
const SIGTERM_NUM: i32 = 15;

/// The concrete per-step cgroup manager for a real Linux cgroup-v2 host.
pub struct Cgroups {
    enabled: bool,
    /// The delegated outer scope cgroup root (parent of the per-step child cgroups).
    root: Option<PathBuf>,
}

impl Cgroups {
    /// Construct the manager. Only meaningful inside the re-exec'd scope; otherwise disabled.
    pub fn new() -> Self {
        let mut cg = Cgroups {
            enabled: false,
            root: None,
        };
        if !in_scope() {
            return cg;
        }
        let scope = match my_cgroup_path() {
            Some(p) if p.is_dir() => p,
            _ => return cg,
        };
        let controllers: HashSet<String> = match read_trim(&scope, "cgroup.controllers") {
            Some(s) => s.split_whitespace().map(String::from).collect(),
            None => {
                warn("outer scope cgroup.controllers unreadable; per-step containment disabled");
                return cg;
            }
        };
        // Drain EVERY process out of the scope root into `supervisor/` so the root holds no
        // processes and may enable controllers for children (cgroup-v2 "no internal processes").
        let sup = scope.join(SUPERVISOR);
        if let Err(e) = make_dir(&sup) {
            warn(&format!(
                "could not create supervisor cgroup {} ({e}); per-step containment disabled",
                sup.display()
            ));
            return cg;
        }
        for _ in 0..5 {
            let pids = read_trim(&scope, "cgroup.procs").unwrap_or_default();
            let pids: Vec<&str> = pids.split_whitespace().collect();
            if pids.is_empty() {
                break;
            }
            for pid in pids {
                let _ = fs::write(sup.join("cgroup.procs"), pid);
            }
        }
        // Enable each controller INDEPENDENTLY (an atomic multi-controller write fails wholesale).
        for c in ["memory", "cpu", "pids"] {
            if controllers.contains(c) {
                if let Err(e) = fs::write(scope.join("cgroup.subtree_control"), format!("+{c}")) {
                    warn(&format!(
                        "could not delegate '{c}' controller to per-step cgroups ({e}); per-step \
                         {c} limits/accounting unavailable (outer scope cap still applies)"
                    ));
                }
            }
        }
        cg.root = Some(scope);
        cg.enabled = true;
        cg
    }

    fn child(&self, tag: &str) -> Option<PathBuf> {
        self.root.as_ref().map(|r| r.join(sanitize(tag)))
    }
}

impl Default for Cgroups {
    fn default() -> Self {
        Cgroups::new()
    }
}

impl CgroupManager for Cgroups {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn prepare_command(
        &self,
        tag: &str,
        cmd: &str,
        mem_max: Option<i64>,
        cpu_count: Option<i64>,
    ) -> String {
        let root = match (&self.root, self.enabled) {
            (Some(r), true) => r,
            _ => return cmd.to_string(),
        };
        let child = root.join(sanitize(tag));
        if let Err(e) = make_dir(&child) {
            warn(&format!(
                "step {tag}: could not create child cgroup {} ({e}); step runs under the outer \
                 cap only",
                child.display()
            ));
            return cmd.to_string();
        }
        // Every step is swapless; also clear any inherited soft cap.
        if let Err(e) = fs::write(child.join("memory.swap.max"), "0") {
            warn(&format!(
                "step {tag}: could not disable swap ({e}); memory controller may not be delegated \
                 — outer cap still applies"
            ));
        }
        let _ = fs::write(child.join("memory.high"), "max");
        // Kill the WHOLE step cgroup as a unit on OOM (`memory.oom.group=1`). Without it the kernel
        // kills one victim process inside whichever cgroup it OOMs: a capped step is left half-dead,
        // and a runaway escalating past its own cap to a shared ancestor gets a victim chosen by
        // badness across ALL steps — so the kill can land on an INNOCENT NEIGHBOUR and be attributed
        // to the wrong PR. Best-effort: a kernel without oom.group must not drop the caps above.
        let _ = fs::write(child.join("memory.oom.group"), "1");
        if let Some(m) = mem_max {
            if let Err(e) = fs::write(child.join("memory.max"), m.to_string()) {
                warn(&format!(
                    "step {tag}: could not apply inner memory cap memory.max={m} ({e}); step runs \
                     under the outer cap only"
                ));
            }
        }
        if let Some(cpu) = cpu_count {
            let period: i64 = 100_000;
            let expected = format!("{} {}", cpu * period, period);
            match fs::write(child.join("cpu.max"), &expected) {
                Ok(()) => {
                    let applied = read_trim(&child, "cpu.max").unwrap_or_default();
                    if applied != expected {
                        return format!(
                            "echo 'ERROR: step {tag} cpu.max mismatch: expected {expected}, got \
                             {applied}' >&2\nexit 1\n"
                        );
                    }
                }
                Err(e) => {
                    return format!(
                        "echo 'ERROR: step {tag} cpu.max could not be applied: {e}' >&2\nexit 1\n"
                    );
                }
            }
        }
        // Carry the build `-j` WITH the caps just written. cargo (and the NUM_JOBS it exports to
        // build scripts) auto-detects parallelism from the effective CPU quota; an UNPINNED step
        // (cpu_count None -> no per-step cpu.max) inherits the wide scope quota and computes
        // NUM_JOBS=<all-granted-cores> (observed 284), OOM-racing the linker (hermit#1584
        // build.dbi_release, 8.0 GiB cap, oom_kill=2). Derive the cap here, where the quota is
        // granted, from the step's cores+mem if pinned else the SCOPE's effective
        // cpu.max/memory.max. Only CARGO_BUILD_JOBS (never MAKEFLAGS): a global make -j could
        // parallelize a determinism-sensitive target (cf. make -jN nondeterminism #1157). An
        // explicit `cargo -j` in the step command still overrides this env floor.
        let eff_cores = cpu_count.or_else(|| cpu_max_cores(read_trim(root, "cpu.max")));
        let eff_mem = mem_max.or_else(|| memory_max_bytes(read_trim(root, "memory.max")));
        let jobs = crate::sizing::derive_build_jobs(eff_cores, eff_mem);
        // $$ is the bash leader's pid; writing it migrates the leader so every subsequently
        // forked descendant inherits this cgroup at fork. Best-effort in the shell.
        let procs = child.join("cgroup.procs");
        format!(
            "echo $$ > {} 2>/dev/null || true\nexport CARGO_BUILD_JOBS={jobs}\n{cmd}",
            procs.display()
        )
    }

    fn kill(&self, tag: &str) -> bool {
        let child = match self.child(tag) {
            Some(c) if self.enabled => c,
            _ => return false,
        };
        match fs::write(child.join("cgroup.kill"), "1") {
            Ok(()) => true,
            Err(e) => {
                warn(&format!(
                    "step {tag}: cgroup.kill write failed ({e}); falling back to process-group \
                     kill for this step"
                ));
                false
            }
        }
    }

    fn cleanup(&self, tag: &str) {
        if let Some(child) = self.child(tag) {
            let _ = fs::remove_dir(child); // EBUSY is fine; the outer-scope stop flushes it
        }
    }

    fn oom_kills(&self, tag: &str) -> i64 {
        let child = match self.child(tag) {
            Some(c) if self.enabled => c,
            _ => return 0,
        };
        let events = match read_trim(&child, "memory.events") {
            Some(e) => e,
            None => return 0,
        };
        for line in events.lines() {
            if let Some(rest) = line.strip_prefix("oom_kill ") {
                return rest.trim().parse().unwrap_or(0);
            }
        }
        0
    }

    fn peak_bytes(&self, tag: &str) -> Option<i64> {
        let child = self.child(tag)?;
        if !self.enabled {
            return None;
        }
        read_trim(&child, "memory.peak").and_then(|s| s.parse().ok())
    }

    fn cpu_stats(&self, tag: &str) -> Option<BTreeMap<String, i64>> {
        let child = self.child(tag)?;
        if !self.enabled {
            return None;
        }
        let text = read_trim(&child, "cpu.stat")?;
        let mut out = BTreeMap::new();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                if let Ok(n) = v.parse::<i64>() {
                    out.insert(k.to_string(), n);
                }
            }
        }
        Some(out)
    }

    fn cpu_pressure(&self, tag: &str) -> Option<BTreeMap<String, f64>> {
        let child = self.child(tag)?;
        if !self.enabled {
            return None;
        }
        let text = read_trim(&child, "cpu.pressure")?;
        let some = text.lines().find(|l| l.starts_with("some "))?;
        let mut out = BTreeMap::new();
        for item in some.split_whitespace().skip(1) {
            if let Some((k, v)) = item.split_once('=') {
                if matches!(k, "avg10" | "avg60") {
                    if let Ok(n) = v.parse::<f64>() {
                        out.insert(k.to_string(), n);
                    }
                }
            }
        }
        // Match the Python reader: both avg10 and avg60 must be present, else None.
        if out.contains_key("avg10") && out.contains_key("avg60") {
            Some(out)
        } else {
            None
        }
    }

    fn thread_count(&self, tag: &str) -> Option<i64> {
        let child = self.child(tag)?;
        if !self.enabled {
            return None;
        }
        let text = read_trim(&child, "cgroup.threads")?;
        Some(text.lines().filter(|l| !l.is_empty()).count() as i64)
    }

    fn kill_all_remaining(&self) -> i64 {
        let root = match (&self.root, self.enabled) {
            (Some(r), true) => r,
            _ => return 0,
        };
        let entries = match fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => return 0,
        };
        let mut n = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let is_step = path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with(STEP_PREFIX));
            if !is_step {
                continue;
            }
            n += 1;
            if let Err(e) = fs::write(path.join("cgroup.kill"), "1") {
                warn(&format!(
                    "backstop: cgroup.kill on {} failed ({e}); a leftover orphan may survive",
                    path.display()
                ));
            }
            let _ = fs::remove_dir(&path);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_makes_safe_names() {
        assert_eq!(sanitize("build.app"), "step-build.app");
        assert_eq!(sanitize("weird/tag name"), "step-weird_tag_name");
    }

    #[test]
    fn disabled_manager_is_a_noop_outside_a_scope() {
        // Constructed outside a scope -> disabled; every method a safe no-op.
        let cg = Cgroups::new();
        if !cg.enabled() {
            assert_eq!(cg.prepare_command("g.j", "true", Some(1), Some(1)), "true");
            assert!(!cg.kill("g.j"));
            assert_eq!(cg.oom_kills("g.j"), 0);
            assert_eq!(cg.peak_bytes("g.j"), None);
            assert_eq!(cg.kill_all_remaining(), 0);
        }
    }

    #[test]
    fn cpu_quota_percent_floor_is_one_core() {
        assert!(cpu_quota_percent(0.90) >= 100);
    }
}
