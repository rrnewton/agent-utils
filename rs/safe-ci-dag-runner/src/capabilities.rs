//! The enforcement manifest, derived from the guards that implement it.
//!
//! The `capabilities` subcommand advertises which containment guards this engine really
//! applies. That manifest used to be a hand-typed JSON string literal, which meant it could claim
//! enforcement that no longer happened: nothing tied the literal to a guard site, so deleting or
//! short-circuiting a guard left the advertisement intact. The py-vs-rs differential compares the
//! two manifests byte-for-byte, which stops the two ENGINES from drifting apart but cannot notice
//! both being wrong together, nor a manifest drifting from the guard sites inside its own engine.
//!
//! So the manifest is generated here instead. [`ENFORCEMENT_REGISTRY`] is the single declaration;
//! [`enforcement_manifest`] serializes it key-sorted, compact and with lowercase booleans, so the
//! two editions agree BY CONSTRUCTION rather than because two people kept two literals in step;
//! and [`is_enforced`] is consulted at the guard site itself, so flipping one `enforced` flag
//! moves the advertisement AND the behaviour together.
//!
//! [`is_enforced`] panics on an unknown key. A guard site that misspells its capability therefore
//! fails loudly at the moment it runs, rather than reading as "not enforced" and silently
//! switching the guard off — which is the exact failure this module exists to prevent.
//!
//! WHICH KEYS ACTUALLY GATE SOMETHING. Four of the nine are consulted at the code that enforces
//! them in THIS engine, and their flags are load-bearing:
//!
//! | key             | guard site that consults `is_enforced`                              |
//! |-----------------|---------------------------------------------------------------------|
//! | `cpu_timeout`   | the scheduler's 1 Hz `cpu.stat` monitor, which reaps over budget     |
//! | `wall_timeout`  | the scheduler's per-step wait deadline                               |
//! | `oom_detection` | the post-step `memory.events` `oom_kill` read                        |
//! | `memory_max`    | the per-step inner `memory.max` write in the cgroup manager          |
//!
//! `pids_guard` gates the per-step `pids.max` write in the companion reference engine, which is
//! the only one with any pids plumbing; this engine has none, which is what `pids_guard: false`
//! says. The remaining four — `cpu_affinity`, `cpu_bandwidth`, `run_timeout` and `write_domains`
//! — are declarations only: the registry records them and the manifest publishes them, but no
//! code consults their flag, so flipping one changes the advertisement WITHOUT changing
//! behaviour. Wiring those is a further step, and saying so here is better than implying a
//! coverage that does not exist.

use std::sync::{Mutex, RwLock};

/// One advertised containment guard.
///
/// `key` is the manifest key (and what a guard site passes to [`is_enforced`]); `enforced` is
/// whether this engine really applies it; `summary` is the human sentence that used to live in
/// the comment above the string literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    /// Manifest key; also what a guard site passes to [`is_enforced`].
    pub key: &'static str,
    /// Whether this engine really applies the guard.
    pub enforced: bool,
    /// Human sentence describing what the guard does.
    pub summary: &'static str,
}

/// Every enforcement guard this engine advertises, and whether it is real. This is the ONLY place
/// the answer is written down: the manifest is generated from it and the guard sites listed in the
/// module docs consult it. The cgroup-dependent guards take effect only under boxing; the boxed
/// smoke tests in each build anchor these declarations to real behaviour wherever a cgroup-v2 +
/// systemd --user scope exists.
pub const ENFORCEMENT_REGISTRY: &[Capability] = &[
    Capability {
        key: "cpu_affinity",
        enforced: true,
        summary: "opt-in --cores K: constrain the WHOLE run tree to K least-busy free cores with \
                  an exact, verified cgroup cpuset; refuse when unavailable",
    },
    Capability {
        key: "cpu_bandwidth",
        enforced: true,
        summary: "boxed run: exact outer cpu.max = --max-cpus x period, read back before \
                  execution",
    },
    Capability {
        key: "cpu_timeout",
        enforced: true,
        summary: "per-step user+system CPU budget (cgroup cpu.stat), reaped over budget",
    },
    Capability {
        key: "memory_max",
        enforced: true,
        summary: "per-step inner memory.max cap (kernel OOM-kills the step at its cap)",
    },
    Capability {
        key: "oom_detection",
        enforced: true,
        summary: "failure attributed to OOM via cgroup memory.events oom_kill count",
    },
    Capability {
        key: "pids_guard",
        enforced: false,
        summary: "per-step PID/thread ceiling: the reference engine's cgroup manager can write \
                  pids.max, but no caller sets the limit and this engine has no pids plumbing at \
                  all, so the write is gated off and nothing applies a PID ceiling",
    },
    Capability {
        key: "run_timeout",
        enforced: true,
        summary: "OUTER wall budget for the WHOLE run: the scheduler cuts in-flight steps and \
                  still reports (works boxed or unboxed); under boxing it is additionally backed \
                  by the scope's systemd RuntimeMaxSec, set strictly later so the reporting bound \
                  fires first",
    },
    Capability {
        key: "wall_timeout",
        enforced: true,
        summary: "per-step wall-clock ceiling (load-dependent; active with or without boxing)",
    },
    Capability {
        key: "write_domains",
        enforced: true,
        summary: "pre-execution closed-vocabulary declaration guard; omission/unknown/duplicate \
                  domains refuse before any node starts when the DAG opts in",
    },
];

/// Bracket support only (see [`with_registry_override`]); `None` means "use the real one".
static OVERRIDE: RwLock<Option<Vec<Capability>>> = RwLock::new(None);
/// Serializes brackets against each other AND against anything that reads the unbracketed
/// registry: the override is process-wide, and a test harness runs test functions on many
/// threads, so without this a bracket's window is visible to an unrelated concurrent reader.
/// [`with_registry_override`] and [`with_registry_pinned`] are the two sides of that agreement.
static OVERRIDE_SERIAL: Mutex<()> = Mutex::new(());

fn with_active<T>(f: impl FnOnce(&[Capability]) -> T) -> T {
    let guard = OVERRIDE.read().unwrap_or_else(|e| e.into_inner());
    match guard.as_deref() {
        Some(active) => f(active),
        None => f(ENFORCEMENT_REGISTRY),
    }
}

/// The machine-readable manifest: key-sorted, compact, lowercase booleans.
///
/// Byte-identical to the companion reference engine's manifest by construction — both
/// serialize the same shape from their own registry, so a guard present in one build and missing
/// from the other shows up as a differing manifest in `cross`.
pub fn enforcement_manifest() -> String {
    with_active(|active| {
        let mut keyed: Vec<(&str, bool)> = active.iter().map(|c| (c.key, c.enforced)).collect();
        keyed.sort_unstable_by_key(|(key, _)| *key);
        let body = keyed
            .iter()
            .map(|(key, enforced)| format!("\"{key}\":{enforced}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{{body}}}")
    })
}

/// Whether this engine really applies the guard named `key`.
///
/// Call this AT the guard site, not near it, so the flag and the behaviour cannot part company.
///
/// # Panics
///
/// For an unknown key. A misspelled capability at a guard site is a bug that must be loud,
/// because the quiet reading of it — "unknown, so not enforced" — would silently disable the very
/// guard the caller was writing.
pub fn is_enforced(key: &str) -> bool {
    with_active(|active| match active.iter().find(|c| c.key == key) {
        Some(c) => c.enforced,
        None => {
            let mut known: Vec<&str> = active.iter().map(|c| c.key).collect();
            known.sort_unstable();
            panic!(
                "unknown enforcement capability {key:?}; a guard site must name a capability \
                 declared in ENFORCEMENT_REGISTRY (known: {})",
                known.join(", ")
            )
        }
    })
}

/// Temporarily flip one capability's `enforced` flag for the duration of `f`. **Brackets only.**
///
/// This exists so a test can prove the coupling this module claims: flip one flag and assert that
/// BOTH the published manifest and the guarded behaviour move. A test that only re-derived the
/// manifest from a registry it had just built itself would be tautological; a test that only
/// checked behaviour would not notice the manifest lying.
///
/// Mirrored by the companion reference engine's `registry_override` context manager.
///
/// # Panics
///
/// For an unknown key, so a stale bracket cannot silently flip nothing.
pub fn with_registry_override<T>(key: &str, enforced: bool, f: impl FnOnce() -> T) -> T {
    assert!(
        ENFORCEMENT_REGISTRY.iter().any(|c| c.key == key),
        "unknown enforcement capability {key:?}"
    );
    let _serial = OVERRIDE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let flipped: Vec<Capability> = ENFORCEMENT_REGISTRY
        .iter()
        .map(|c| {
            if c.key == key {
                Capability { enforced, ..*c }
            } else {
                *c
            }
        })
        .collect();
    // Restore on the way out even if `f` panics, so one failing bracket cannot leave a
    // process-wide registry flipped under every test that follows it.
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            *OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
    *OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = Some(flipped);
    let _restore = Restore;
    f()
}

/// Run `f` with the real registry held steady against a concurrent [`with_registry_override`].
/// **Brackets only.**
///
/// A bracket's flip is process-wide, so a test asserting what the UNBRACKETED registry says has to
/// take the same turn as the brackets do, or it can read another test's window and fail for a
/// reason that has nothing to do with it.
pub fn with_registry_pinned<T>(f: impl FnOnce() -> T) -> T {
    let _serial = OVERRIDE_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest is written out LITERALLY here, not re-derived from the registry: a test that
    /// serialized the registry and compared it to the registry would pass no matter what the
    /// registry said. This is the byte string the `capabilities` subcommand promises, and the
    /// companion reference engine must print the same one.
    #[test]
    fn manifest_is_exactly_the_published_bytes() {
        let manifest = with_registry_pinned(enforcement_manifest);
        assert_eq!(
            manifest,
            "{\"cpu_affinity\":true,\"cpu_bandwidth\":true,\"cpu_timeout\":true,\
             \"memory_max\":true,\"oom_detection\":true,\"pids_guard\":false,\
             \"run_timeout\":true,\"wall_timeout\":true,\"write_domains\":true}"
        );
    }

    #[test]
    fn manifest_moves_when_a_flag_is_flipped() {
        let flipped = with_registry_override("memory_max", false, enforcement_manifest);
        assert!(flipped.contains("\"memory_max\":false"), "{flipped}");
        let restored = with_registry_pinned(enforcement_manifest);
        assert!(restored.contains("\"memory_max\":true"), "{restored}");
    }

    #[test]
    fn is_enforced_reads_the_registry_and_the_override() {
        assert!(is_enforced("cpu_timeout"));
        assert!(!is_enforced("pids_guard"));
        assert!(!with_registry_override("cpu_timeout", false, || {
            is_enforced("cpu_timeout")
        }));
        assert!(is_enforced("cpu_timeout"));
    }

    #[test]
    #[should_panic(expected = "unknown enforcement capability \"cpu_timout\"")]
    fn a_misspelled_guard_site_panics_instead_of_reading_as_unenforced() {
        with_registry_pinned(|| is_enforced("cpu_timout"));
    }

    #[test]
    fn a_panicking_bracket_still_restores_the_registry() {
        let caught = std::panic::catch_unwind(|| {
            with_registry_override("memory_max", false, || panic!("bracket body blew up"));
        });
        assert!(caught.is_err());
        with_registry_pinned(|| assert!(is_enforced("memory_max")));
    }
}
