---
title: 'launcher-fast-path: fast verified warm path for the Rust checkout launcher'
status: open
priority: 2
issue_type: feature
created_at: 2026-10-29T12:27:42.759715838+00:00
updated_at: 2026-10-29T12:27:42.759715838+00:00
---

# Description

[watermelon-full-preview] Owner request 2026-10-29: the checkout launcher for Rust tools (common/bin/engine-resolver -> rs/bin/cargo-runner) should have a fast warm path so teams can run tools straight from their own pinned checkout, with no explicit compile step and no version skew.

Measured today: a cached call with nothing changed takes ~0.44 s (`agentctl --version` 436-443 ms). Breakdown verified in the slot: a no-op `cargo build` runs on every call even when the provenance stamp is already verified (~100-150 ms); common/bin/rs-source-fingerprint (content hash of ~535 files under rs/, ~60 ms) runs ~4-5 times per call; sha256sum of the 15 MB binary 3-4 times (~13-17 ms each), plus `rustc -vV` (~16-19 ms) and bash startup; a checkout-wide flock serializes parallel calls (8 parallel calls = 3.3 s).

Target: a verified warm call in about 30-50 ms or less, parallel calls not serialized on the warm path, while keeping the fail-closed guarantees: never run a binary whose source differs from the working tree (including untracked files under rs/), never trust cargo's timestamps alone, and keep the snapshot/current_exe behaviour that dagrun and cpuset-alloc need.

Plan: when the stamp verifies cheaply, skip cargo and the redundant fingerprints and exec the existing snapshot; make the fingerprint cheap on a clean tree (git tree id of HEAD:rs plus an empty `git status --porcelain -- rs`, which uses git's stat cache) and fall back to the full content hash only when dirty or unverifiable; cache the host triple; take the lock only on the slow path (re-verify under the lock before building).

Staleness note: if the checkout is used as a team's pinned submodule, the warm path must still notice a pin move (HEAD/tree change) and rebuild automatically.

Measure before/after (cold, warm, warm after an edit, 8 parallel warm calls) and put the table in the commit body and handoff. Keep the Python edition and other tools' behaviour unchanged; update launcher docs/help if behaviour is described there.
