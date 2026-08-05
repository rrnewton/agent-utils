//! Cadence decisions and the crash-safe fired-state store.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::model::{Reminder, EVERY_TICK};
use crate::text::{is_whitespace, split_lines, trim};

/// Return whether a reminder should be checked at `now`.
pub fn is_due(name: &str, cadence_secs: i64, now: i64, last_fired: &BTreeMap<String, i64>) -> bool {
    if cadence_secs <= EVERY_TICK {
        return true;
    }
    last_fired
        .get(name)
        .is_none_or(|last| now.saturating_sub(*last) >= cadence_secs)
}

/// Return due reminders in registration order.
pub fn due_reminders<'a>(
    reminders: &'a [Reminder],
    now: i64,
    last_fired: &BTreeMap<String, i64>,
) -> Vec<&'a Reminder> {
    reminders
        .iter()
        .filter(|reminder| is_due(&reminder.name, reminder.cadence_secs, now, last_fired))
        .collect()
}

/// Load valid `key=epoch` lines. Missing, unreadable, and malformed data is ignored.
pub fn load_fired_state(path: &Path) -> BTreeMap<String, i64> {
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let mut state = BTreeMap::new();
    for raw_line in split_lines(&text) {
        let line = trim(raw_line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.is_empty()
            || key.chars().any(is_whitespace)
            || value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        if let Ok(epoch) = value.parse::<i64>() {
            state.insert(key.to_string(), epoch);
        }
    }
    state
}

/// Atomically write a sorted fired-state file using a same-directory temporary file.
pub fn persist_fired_state(path: &Path, state: &BTreeMap<String, i64>) -> io::Result<()> {
    for (key, value) in state {
        if key.is_empty() || key.contains('=') || key.chars().any(is_whitespace) || *value < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid fired-state entry {key:?}={value}"),
            ));
        }
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut temporary_name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("state"))
        .to_os_string();
    temporary_name.push(".tmp");
    let temporary = path.with_file_name(temporary_name);
    let mut text =
        String::from("# tick-hub fired-state — key=last_fired_epoch (managed by tick-hub)\n");
    for (key, value) in state {
        text.push_str(&format!("{key}={value}\n"));
    }
    fs::write(&temporary, text)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Emit, Reminder};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tick-hub-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn exact_cadence_boundary_is_due() {
        let fired = BTreeMap::from([("r".to_string(), 1_000)]);
        assert!(is_due("r", 600, 1_600, &fired));
        assert!(!is_due("r", 600, 1_599, &fired));
        assert!(is_due("r", 0, 1_000, &fired));
    }

    #[test]
    fn due_reminders_preserve_registration_order() {
        let mut a = Reminder::new("a", Emit::note("a"));
        a.cadence_secs = 0;
        let mut b = Reminder::new("b", Emit::note("b"));
        b.cadence_secs = 3_600;
        let c = Reminder::new("c", Emit::note("c"));
        let fired = BTreeMap::from([("b".to_string(), 1_000)]);
        let reminders = [a, b, c];
        let names: Vec<_> = due_reminders(&reminders, 1_500, &fired)
            .into_iter()
            .map(|reminder| reminder.name.as_str())
            .collect();
        assert_eq!(names, ["a", "c"]);
    }

    #[test]
    fn fired_state_round_trips_and_ignores_garbage() {
        let root = temporary_path("cadence");
        let path = root.join("sub/state");
        let state = BTreeMap::from([("a".to_string(), 100), ("b".to_string(), 200)]);
        persist_fired_state(&path, &state).unwrap();
        assert_eq!(load_fired_state(&path), state);
        fs::write(&path, "# comment\nvalid=42\nbad line\nk=notnum\n").unwrap();
        assert_eq!(
            load_fired_state(&path),
            BTreeMap::from([("valid".into(), 42)])
        );
        assert!(persist_fired_state(&path, &BTreeMap::from([("bad key".into(), 1)]),).is_err());
        assert!(persist_fired_state(&path, &BTreeMap::from([("bad".into(), -1)]),).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fired_state_removes_temporary_file_after_rename_failure() {
        let root = temporary_path("cadence-rename-failure");
        let path = root.join("state");
        fs::create_dir_all(&path).unwrap();
        assert!(persist_fired_state(&path, &BTreeMap::from([("valid".into(), 1)])).is_err());
        assert!(!root.join("state.tmp").exists());
        let _ = fs::remove_dir_all(root);
    }
}
