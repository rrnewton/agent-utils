//! Enforcement-capability registry: the SINGLE SOURCE OF TRUTH for the manifest.
//!
//! The `capabilities` subcommand does NOT print a hand-maintained JSON literal. It serializes this
//! registry ([`enforcement_manifest`]), and every enforcement guard in the runner consults
//! [`is_enforced`] at its guard site (`cpu_timeout` reap, `wall_timeout` wait deadline,
//! `oom_detection` event read, inner `memory_max` write, `solo_validate` admission), so the
//! ADVERTISED manifest and the CODE that enforces it read the SAME source and cannot silently
//! diverge. This is the recurrence guard for the historical gap where the Rust runner silently did
//! NOT enforce `cpu_timeout` while the Python runner did.
//!
//! The manifest is derived, not declared: flip a [`Capability`]'s `enforced` flag and BOTH the
//! emitted manifest AND the guarded behavior change together (a guard wrapped in `is_enforced(key)`
//! becomes inert when its capability is flagged off). The py-vs-rs differential asserts the two
//! engines' serialized manifests are byte-identical and reports N = [`capability_count`].
//!
//! MUST stay behaviorally identical to `py/safe_ci_dag_runner/capabilities.py`.

/// One enforcement guard the engine advertises. `enforced` is the truth the guard site reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub key: &'static str,
    pub enforced: bool,
    pub summary: &'static str,
}

/// The enumerated enforcement guards, in key order. This is the source of truth the manifest is
/// generated from and that every guard site consults; it is NOT a description of a literal kept
/// elsewhere.
///   cpu_timeout    per-step user+system CPU budget (cgroup cpu.stat), reaped over budget
///   memory_max     per-step inner memory.max cap (kernel OOM-kills the step at its cap)
///   oom_detection  failure attributed to OOM via cgroup memory.events oom_kill count
///   pids_guard     per-step PID/thread ceiling (plumbed in both, enforced in neither -> false)
///   solo_validate  SOLO-VALIDATE box exclusivity: a validate node is refused admission while
///                  another validate OR a benchmark harness holds the box (see [`crate::admission`])
///   wall_timeout   per-step wall-clock ceiling (load-dependent; active with or without boxing)
pub const ENFORCEMENT_REGISTRY: &[Capability] = &[
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
        summary: "per-step PID/thread ceiling (plumbed in both, enforced in neither)",
    },
    Capability {
        key: "solo_validate",
        enforced: true,
        summary:
            "SOLO-VALIDATE box exclusivity: a validate node is refused admission while another \
                  validate or a benchmark harness holds the box",
    },
    Capability {
        key: "wall_timeout",
        enforced: true,
        summary: "per-step wall-clock ceiling (load-dependent; active with or without boxing)",
    },
];

/// Serialize the registry to the compact, key-sorted JSON the `capabilities` subcommand emits.
///
/// Byte-identical across engines by construction: keys sorted, no whitespace, lowercase booleans.
pub fn enforcement_manifest() -> String {
    let mut caps: Vec<&Capability> = ENFORCEMENT_REGISTRY.iter().collect();
    caps.sort_by_key(|c| c.key);
    let parts: Vec<String> = caps
        .iter()
        .map(|c| {
            format!(
                "\"{}\":{}",
                c.key,
                if c.enforced { "true" } else { "false" }
            )
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// Whether guard `key` is actively enforced.
///
/// Panics for an unknown key so a typo at a guard site fails LOUDLY rather than silently disabling
/// enforcement (mirrors the Python engine's `KeyError`).
pub fn is_enforced(key: &str) -> bool {
    ENFORCEMENT_REGISTRY
        .iter()
        .find(|c| c.key == key)
        .unwrap_or_else(|| panic!("unknown enforcement capability: {key}"))
        .enforced
}

/// N: the number of declared enforcement capabilities (reported by the differential).
pub fn capability_count() -> usize {
    ENFORCEMENT_REGISTRY.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_key_sorted_compact_json() {
        // Derived, not literal: exactly the guards in ENFORCEMENT_REGISTRY, sorted, compact.
        assert_eq!(
            enforcement_manifest(),
            "{\"cpu_timeout\":true,\"memory_max\":true,\"oom_detection\":true,\
             \"pids_guard\":false,\"solo_validate\":true,\"wall_timeout\":true}"
        );
        assert_eq!(capability_count(), 6);
    }

    #[test]
    fn is_enforced_reads_the_registry() {
        assert!(is_enforced("cpu_timeout"));
        assert!(is_enforced("solo_validate"));
        assert!(!is_enforced("pids_guard"));
    }

    #[test]
    #[should_panic(expected = "unknown enforcement capability")]
    fn unknown_key_panics_loudly() {
        is_enforced("no_such_guard");
    }
}
