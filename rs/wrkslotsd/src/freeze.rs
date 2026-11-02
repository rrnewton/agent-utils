//! Per-host retire freeze (`freeze`, #780): while set, every
//! retirement/removal refuses with the stable `retire-freeze` code.

use std::path::Path;

use serde_json::Value;

use crate::active_state::{load_cfg, Cfg};
use crate::config_validate::load_config;

type Result<T> = std::result::Result<T, String>;

pub(crate) const MARKER: &str = "RETIRE_FREEZE";
pub(crate) const CODE: &str = "retire-freeze";

/// (reason, source) when frozen: config key wins, else the marker file.
pub(crate) fn retire_freeze(cfg: &Cfg, config: &Value) -> Option<(String, String)> {
    if let Some(reason) = config.get("retire_freeze").and_then(Value::as_str) {
        if !reason.is_empty() {
            return Some((reason.to_string(), "config".to_string()));
        }
    }
    let text = std::fs::read_to_string(cfg.control.join(MARKER)).ok()?;
    let reason = text.trim();
    Some((
        if reason.is_empty() {
            "frozen by marker file".to_string()
        } else {
            reason.to_string()
        },
        "marker".to_string(),
    ))
}

pub(crate) fn assert_not_frozen(cfg: &Cfg, config: &Value, action: &str) -> Result<()> {
    if let Some((reason, source)) = retire_freeze(cfg, config) {
        return Err(format!(
            "{CODE}: {action} refused on this host (freeze source: {source}): {reason}"
        ));
    }
    Ok(())
}

/// `freeze set|clear|status` over the marker file.
pub(crate) fn freeze_command(
    root: Option<&Path>,
    machine_override: Option<&str>,
    action: &str,
    reason_words: &[String],
) -> Result<String> {
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    let marker = cfg.control.join(MARKER);
    match action {
        "set" => {
            let reason = reason_words.join(" ").trim().to_string();
            if reason.is_empty() {
                return Err("freeze set requires a non-empty reason".to_string());
            }
            std::fs::create_dir_all(&cfg.control)
                .map_err(|e| format!("cannot create control directory: {e}"))?;
            let temporary = cfg.control.join("RETIRE_FREEZE.tmp");
            std::fs::write(&temporary, format!("{reason}\n"))
                .map_err(|e| format!("cannot write freeze marker: {e}"))?;
            std::fs::rename(&temporary, &marker)
                .map_err(|e| format!("cannot write freeze marker: {e}"))?;
            Ok(format!("frozen=true reason={reason} source=marker\n"))
        }
        "clear" => {
            let _ = std::fs::remove_file(&marker);
            match retire_freeze(&cfg, &config) {
                Some((reason, source)) => Ok(format!(
                    "frozen=true reason={reason} source={source} (marker cleared; the remaining source still freezes this host)\n"
                )),
                None => Ok("frozen=false\n".to_string()),
            }
        }
        _ => match retire_freeze(&cfg, &config) {
            Some((reason, source)) => Ok(format!("frozen=true reason={reason} source={source}\n")),
            None => Ok("frozen=false\n".to_string()),
        },
    }
}
