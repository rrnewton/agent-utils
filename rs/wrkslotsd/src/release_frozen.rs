//! Command: release one incomplete retained frozen validation
//! checkout on an operator's word (`release-frozen-incomplete`,
//! #1089). Step 4a ports the read-only half: the reason and
//! coordinator gates, the retention classification, and the
//! dry-run plan payload. The `--yes` salvage and journaled
//! removal execution stays with the Python edition (step 4b)
//! and refuses explicitly rather than approximating it.
//! Fixture registries only until the edition cutover.

use std::path::Path;

use serde_json::Value;

use crate::active_state::load_cfg;
use crate::config_validate::validate_name;
use crate::ensure_states::require_fixture;

type Result<T> = std::result::Result<T, String>;

/// Python `_frozen_release_reason`.
fn release_reason(raw: &str) -> Result<String> {
    let reason = raw.trim();
    if reason.is_empty() {
        return Err(
            "release-frozen-incomplete requires --reason saying why this checkout holds nothing anyone needs; nothing was read or changed"
                .to_string(),
        );
    }
    if reason.chars().count() > 2000 || reason.contains('\0') {
        return Err("--reason must be at most 2000 characters without NUL bytes".to_string());
    }
    Ok(reason.to_string())
}

/// Run the dry-run release classification and plan; `--yes`
/// refuses before anything is read or changed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn release_frozen_incomplete(
    root: Option<&Path>,
    machine_override: Option<&str>,
    frozen_checkout: &str,
    completed_record: &str,
    repository: &str,
    reason: &str,
    yes: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    let reason = release_reason(reason)?;
    if !coordinator_authorized {
        return Err(
            "frozen validation release is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized"
                .to_string(),
        );
    }
    require_fixture()?;
    let pid = coordinator_pid.ok_or("frozen validation release requires --coordinator-pid")?;
    crate::status_state::read_process_identity(pid)?;
    if yes {
        return Err(
            "frozen validation release execution stays with the Python edition for now".to_string(),
        );
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    let config = crate::config_validate::load_config(root, machine_override)?;
    crate::ensure_states::refuse_partial_state_allow_seals(&cfg.control)?;
    if !crate::classify_journals::outstanding_journal_paths(&cfg)?.is_empty() {
        return Err(
            "an interrupted mutation is already recorded; recover it before releasing a frozen validation checkout"
                .to_string(),
        );
    }
    crate::validation_recovery::frozen_release_dry_run(
        &cfg,
        &config,
        frozen_checkout,
        completed_record,
        repository,
        &reason,
    )
}

/// The Python human rendering of one release payload.
pub(crate) fn release_text(payload: &Value) -> String {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let number = |key: &str| payload.get(key).and_then(Value::as_u64).unwrap_or_default();
    if text("action") == "dry-run" {
        let mut out = format!(
            "DRY RUN: would release {} ({} bytes, {} files)\n  record={} target={}\n",
            text("checkout"),
            number("bytes"),
            number("files"),
            text("record"),
            text("target"),
        );
        if let Some(repositories) = payload.get("repositories").and_then(Value::as_array) {
            for row in repositories {
                let held = row.get("on_remote").and_then(Value::as_bool) == Some(true);
                out.push_str(&format!(
                    "  {} {}: {}\n",
                    row.get("repository")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    row.get("head").and_then(Value::as_str).unwrap_or_default(),
                    if held { "on remote" } else { "would bundle" },
                ));
            }
        }
        out.push_str("nothing changed; rerun with --yes to salvage and remove\n");
        out
    } else {
        format!(
            "released {} ({} bytes); note {}\n",
            text("checkout"),
            number("bytes"),
            text("note")
        )
    }
}
