//! A host-wide budget for the rescan's listing calls, shared by every bridge that lists as the
//! same Slack identity (#690 muse-bridge-rescan-intake).
//!
//! Every bridge on a host reads its channel as the same person, through the same client, and
//! Slack (and the client's own limiter) counts their listings together: on 2026-10-29 several
//! bridges rescanning at once got the owner's history and replies reads rate-limited, which
//! slowed the owner's other Slack tools. So the bridges share one small state file per identity
//! under the user's runtime directory, `$XDG_RUNTIME_DIR/agentctl-slack/budget-<id>.json`
//! (`/run/user/<uid>` when the variable is unset), read and written under an exclusive `flock`:
//!
//! - `window_start_millis` and `used`: listing calls made in the current minute, at most
//!   `per_minute` of them across all bridges;
//! - `blocked_until_millis`: no bridge lists before then; set by any bridge that was told to
//!   back off (a rate-limited answer), so the others back off with it.
//!
//! The file holds counters only, never message text or identifiers. A budget that cannot be read
//! or written refuses nothing: it is advisory, and the rescan's own rate-limit handling still
//! applies.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// One minute, the budget's window.
const WINDOW_MILLIS: u64 = 60_000;
/// The shortest shared back-off after a rate-limited answer.
pub const MIN_BLOCK_MILLIS: u64 = 30_000;

/// Why the budget refused a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A bridge was told to back off; no listing before this instant (milliseconds).
    Blocked(u64),
    /// This minute's calls are spent; the next window opens at this instant (milliseconds).
    Spent(u64),
}

/// The calls one identity made in the current window, as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Calls made in the current minute, across bridges.
    pub used: u64,
    /// The per-minute allowance.
    pub per_minute: u64,
    /// Whether the budget file could be used; when not, nothing was counted or refused.
    pub available: bool,
}

/// The shared budget of one identity.
#[derive(Clone, Debug)]
pub struct CallBudget {
    path: PathBuf,
    per_minute: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct State {
    window_start_millis: u64,
    used: u64,
    blocked_until_millis: u64,
}

impl CallBudget {
    /// The budget named `identity` (a bridge's `rescan_budget_key`) for the current user, hashed
    /// into the file name under the runtime directory; an absolute directory instead keeps the
    /// budget in `budget.json` there. `None` for the key `off`, or when no runtime directory is
    /// known.
    #[must_use]
    pub fn for_identity(identity: &str, per_minute: u64) -> Option<Self> {
        if identity == "off" {
            return None;
        }
        // An absolute directory names where the budget lives, for a budget of its own.
        if identity.starts_with('/') {
            return Some(Self::at(
                &Path::new(identity).join("budget.json"),
                per_minute,
            ));
        }
        let directory = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                let uid = current_uid();
                let path = PathBuf::from(format!("/run/user/{uid}"));
                path.is_dir().then_some(path)
            })?
            .join("agentctl-slack");
        Some(Self::at(
            &directory.join(format!("budget-{}.json", fingerprint(identity))),
            per_minute,
        ))
    }

    /// A budget kept in the file `path`.
    #[must_use]
    pub fn at(path: &Path, per_minute: u64) -> Self {
        Self {
            path: path.to_owned(),
            per_minute,
        }
    }

    /// Spend one call at `now` (milliseconds), or say why not.
    ///
    /// # Errors
    ///
    /// Returns the [`Refusal`]; a budget file that cannot be used refuses nothing.
    pub fn take(&self, now: u64) -> Result<Usage, Refusal> {
        self.spend(now, true)
    }

    /// As [`Self::take`], but not held by a back-off: for the retry, after the wait Slack named,
    /// of the very listing whose rate limit recorded that back-off for the other bridges.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::Spent`] when this minute's calls are spent.
    pub fn take_ignoring_back_off(&self, now: u64) -> Result<Usage, Refusal> {
        self.spend(now, false)
    }

    fn spend(&self, now: u64, held_by_back_off: bool) -> Result<Usage, Refusal> {
        let per_minute = self.per_minute;
        let mut outcome = Ok(Usage::default());
        let updated = self.update(|state| {
            if held_by_back_off && now < state.blocked_until_millis {
                outcome = Err(Refusal::Blocked(state.blocked_until_millis));
                return false;
            }
            if state.used == 0
                || now.saturating_sub(state.window_start_millis) >= WINDOW_MILLIS
                || now < state.window_start_millis
            {
                state.window_start_millis = now;
                state.used = 0;
            }
            if per_minute > 0 && state.used >= per_minute {
                outcome = Err(Refusal::Spent(
                    state.window_start_millis.saturating_add(WINDOW_MILLIS),
                ));
                return false;
            }
            state.used += 1;
            outcome = Ok(Usage {
                used: state.used,
                per_minute,
                available: true,
            });
            true
        });
        if updated.is_err() {
            // Advisory: a budget file that cannot be used refuses nothing, and says so.
            return Ok(Usage {
                per_minute,
                ..Usage::default()
            });
        }
        outcome
    }

    /// Make every bridge of this identity back off until `until` (milliseconds); an earlier
    /// instant than one already set changes nothing.
    pub fn block(&self, until: u64) {
        let _ = self.update(|state| {
            if until > state.blocked_until_millis {
                state.blocked_until_millis = until;
                true
            } else {
                false
            }
        });
    }

    /// The calls made in the current window as of `now`, without spending one.
    #[must_use]
    pub fn usage(&self, now: u64) -> Usage {
        let mut usage = Usage {
            used: 0,
            per_minute: self.per_minute,
            available: false,
        };
        let updated = self.update(|state| {
            if now.saturating_sub(state.window_start_millis) < WINDOW_MILLIS {
                usage.used = state.used;
            }
            false
        });
        usage.available = updated.is_ok();
        usage
    }

    /// Read the state under the lock, let `change` edit it, and write it back when `change`
    /// returns `true`.
    fn update(&self, change: impl FnOnce(&mut State) -> bool) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&self.path)?;
        lock_exclusive(&file)?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let mut state = parse(&text);
        if change(&mut state) {
            let bytes = serde_json::to_vec(&json!({
                "window_start_millis": state.window_start_millis,
                "used": state.used,
                "blocked_until_millis": state.blocked_until_millis,
            }))
            .map_err(std::io::Error::other)?;
            file.seek(SeekFrom::Start(0))?;
            file.set_len(0)?;
            file.write_all(&bytes)?;
        }
        // Closing the file releases the lock.
        Ok(())
    }

    /// The file this budget is kept in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn parse(text: &str) -> State {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return State::default();
    };
    let number = |key: &str| value[key].as_u64().unwrap_or(0);
    State {
        window_start_millis: number("window_start_millis"),
        used: number("used"),
        blocked_until_millis: number("blocked_until_millis"),
    }
}

/// A short stable name for `identity` and the current user: FNV-1a, 64 bits, in hex.
fn fingerprint(identity: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in identity
        .as_bytes()
        .iter()
        .chain(b"\0")
        .chain(current_uid().to_string().as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[allow(unsafe_code)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` takes no arguments, reads no memory and cannot fail.
    unsafe { libc::getuid() }
}

#[allow(unsafe_code)]
fn lock_exclusive(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: `flock` takes a descriptor `file` owns for the call and flags; it reads no memory.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "agentctl-slack-budget-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        directory.join("budget.json")
    }

    #[test]
    fn calls_are_counted_per_minute_across_handles_of_one_file() {
        let path = scratch("count");
        let one = CallBudget::at(&path, 2);
        let other = CallBudget::at(&path, 2);
        assert_eq!(one.take(1_000).unwrap().used, 1);
        assert_eq!(other.take(2_000).unwrap().used, 2);
        assert_eq!(one.take(3_000), Err(Refusal::Spent(61_000)));
        assert_eq!(other.usage(3_000).used, 2);
        // A new minute starts the count again.
        assert_eq!(other.take(61_000).unwrap().used, 1);
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_back_off_set_by_one_bridge_holds_every_bridge() {
        let path = scratch("block");
        let one = CallBudget::at(&path, 0);
        let other = CallBudget::at(&path, 0);
        one.block(50_000);
        assert_eq!(other.take(10_000), Err(Refusal::Blocked(50_000)));
        // The bridge that recorded it retries after Slack's wait regardless.
        assert!(one.take_ignoring_back_off(11_000).unwrap().available);
        // An earlier instant does not shorten it.
        other.block(20_000);
        assert_eq!(one.take(40_000), Err(Refusal::Blocked(50_000)));
        // Zero per minute is unlimited once the back-off ends.
        for at in 50_000..50_010 {
            assert!(one.take(at).is_ok());
        }
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn an_unreadable_state_counts_from_zero_and_the_name_is_stable() {
        let path = scratch("garbage");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json").unwrap();
        assert_eq!(CallBudget::at(&path, 5).take(1).unwrap().used, 1);
        assert_eq!(fingerprint("/a/client"), fingerprint("/a/client"));
        assert_ne!(fingerprint("/a/client"), fingerprint("/b/client"));
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
