//! One bridge per channel (#677 chat-bridge-repoint). `chat run` holds a host-wide lock for each
//! configured channel for as long as it runs, so a second `chat run` on a channel that already has
//! a live bridge is refused at startup, whichever state directory or registry it uses. Two
//! bridges on one channel each admit every message, and the second can type them into another
//! agent's pane.
//!
//! The locks live in one directory of their own, one file per channel, named by a digest of the
//! subscription plugin and channel ID. Each holds `flock` while its bridge runs, so a bridge that
//! dies releases its locks with its last descriptor; the file's text names the holder for the
//! refusal and stays behind harmlessly.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::chat_runtime::{BridgeConfiguration, ChatRuntimeError};

use super::ChatServiceError;

/// Environment variable naming the directory of channel locks, for an isolated installation or
/// a test; unset or empty selects [`default_lock_directory`].
pub const CHANNEL_LOCK_DIR_ENV: &str = "AGENTCTL_CHAT_CHANNEL_LOCK_DIR";

/// The channel locks one `chat run` holds until it drops them, at exit.
#[derive(Debug)]
pub(super) struct ChannelLocks {
    _files: Vec<File>,
}

/// The directory of channel locks: `AGENTCTL_CHAT_CHANNEL_LOCK_DIR` when set, else
/// [`default_lock_directory`].
#[must_use]
pub fn lock_directory() -> PathBuf {
    lock_directory_from(env::var_os(CHANNEL_LOCK_DIR_ENV))
}

fn lock_directory_from(configured: Option<OsString>) -> PathBuf {
    configured
        .filter(|value| !value.is_empty())
        .map_or_else(default_lock_directory, PathBuf::from)
}

/// `/run/user/UID/agentctl/chat-channels` when the user's runtime directory exists, else
/// `/tmp/agentctl-UID/chat-channels`. Neither depends on the environment, so a service unit and a
/// shell of the same user agree on them while the runtime directory exists.
#[must_use]
pub fn default_lock_directory() -> PathBuf {
    // SAFETY: geteuid takes no arguments and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    if std::fs::metadata(&runtime).is_ok_and(|metadata| metadata.is_dir()) {
        runtime.join("agentctl").join("chat-channels")
    } else {
        PathBuf::from(format!("/tmp/agentctl-{uid}")).join("chat-channels")
    }
}

/// The lock file of `channel` on `plugin` in `directory`.
pub(super) fn lock_path(directory: &Path, plugin: &str, channel: &str) -> PathBuf {
    let digest = Sha256::digest(format!("{plugin}\n{channel}").as_bytes());
    let name: String = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    directory.join(format!("{name}.lock"))
}

/// Create `directory` and each missing parent below an existing one as private directories of
/// this user, refusing any part that another user owns or can write.
fn prepare(directory: &Path) -> Result<(), ChatServiceError> {
    let mut missing = Vec::new();
    let mut cursor = directory;
    while std::fs::symlink_metadata(cursor).is_err() {
        missing.push(cursor.to_path_buf());
        match cursor.parent() {
            Some(parent) => cursor = parent,
            None => break,
        }
    }
    for path in missing.iter().rev() {
        crate::agent::create_private_directory(path, "chat channel lock directory", false, true)?;
    }
    crate::agent::validate_private_directory(directory, "chat channel lock directory", true)?;
    Ok(())
}

/// Take the lock of every channel `configuration` names for the bridge serving `state_root`, or
/// refuse, naming the bridge that holds one. Nothing is held after a refusal.
pub(super) fn acquire(
    state_root: &Path,
    configuration: &BridgeConfiguration,
) -> Result<ChannelLocks, ChatServiceError> {
    acquire_in(&lock_directory(), state_root, configuration)
}

pub(super) fn acquire_in(
    directory: &Path,
    state_root: &Path,
    configuration: &BridgeConfiguration,
) -> Result<ChannelLocks, ChatServiceError> {
    prepare(directory)?;
    let state = std::path::absolute(state_root).unwrap_or_else(|_| state_root.to_path_buf());
    let mut files = Vec::with_capacity(configuration.channel_ids.len());
    for channel in &configuration.channel_ids {
        let path = lock_path(directory, &configuration.subscription_plugin, channel);
        let mut file = crate::agent::open_private_lock(&path, "chat channel lock")?;
        if let Err(error) = file.try_lock_exclusive() {
            if error.kind() != io::ErrorKind::WouldBlock
                && error.raw_os_error() != Some(libc::EWOULDBLOCK)
            {
                return Err(ChatServiceError::Io(error));
            }
            return Err(refusal(channel, &path, &mut file, &state));
        }
        let holder = json!({
            "pid": std::process::id(),
            "bridge_state": state.display().to_string(),
            "subscription_plugin": configuration.subscription_plugin,
            "channel_id": channel,
            "started_at_millis": crate::chat_runtime::unix_millis(),
        });
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(format!("{holder}\n").as_bytes())?;
        files.push(file);
    }
    Ok(ChannelLocks { _files: files })
}

/// The refusal for `channel`, whose lock at `path` another bridge holds, naming that bridge from
/// the lock's text and `/proc/locks` where they can be read.
fn refusal(channel: &str, path: &Path, file: &mut File, state: &Path) -> ChatServiceError {
    let mut text = String::new();
    let _ = file.take(4_096).read_to_string(&mut text);
    let recorded: Option<Value> = serde_json::from_str(text.trim()).ok();
    let holder_state = recorded
        .as_ref()
        .and_then(|value| value["bridge_state"].as_str())
        .map(str::to_owned);
    let pid = crate::lock_wait::holder_of(path)
        .or_else(|| recorded.as_ref().and_then(|value| value["pid"].as_u64()));
    let who = match (pid, holder_state.as_deref()) {
        (Some(pid), Some(other)) => format!("process {pid}, serving bridge state {other}"),
        (Some(pid), None) => format!("process {pid}"),
        (None, Some(other)) => format!("the bridge serving state {other}"),
        (None, None) => "another chat run".to_owned(),
    };
    let same = holder_state.as_deref() == Some(&*state.display().to_string());
    let remedy = if same {
        "another chat run already serves this state directory; stop it before starting this one"
            .to_owned()
    } else {
        format!(
            "stop that bridge first, or point it at another agent without a restart with \
             `agentctl chat repoint --bridge-state {} --agent NAME`",
            holder_state.as_deref().unwrap_or("DIR")
        )
    };
    ChatServiceError::Runtime(ChatRuntimeError::Invalid(format!(
        "channel {channel} already has a live bridge ({who}; lock {}): only one bridge may \
         serve a channel; {remedy}",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A private scratch directory, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
            let path = env::temp_dir().join(format!(
                "agentctl-channel-lock-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            crate::agent::create_private_directory(&path, "test scratch", false, true)
                .expect("scratch directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn configuration(channels: &[&str]) -> BridgeConfiguration {
        BridgeConfiguration {
            subscription_plugin: "fake".to_owned(),
            subscription_environment: Vec::new(),
            channel_ids: channels
                .iter()
                .map(|channel| (*channel).to_owned())
                .collect(),
            allowed_senders: vec!["users/owner".to_owned()],
            max_grant_seconds: crate::chat_runtime::DEFAULT_MAX_GRANT_SECONDS,
            agent_name: "coordinator".to_owned(),
            agent_label: "Coordinator".to_owned(),
            outbound_enabled: false,
            ack_reaction: Default::default(),
            receipt_reaction: Default::default(),
            reject_reaction: Default::default(),
            backend_configuration: None,
            outbound_command: None,
            reply_alias_range: None,
            slack_format: None,
            slack_format_by_channel: Default::default(),
        }
    }

    #[test]
    fn a_second_bridge_on_a_channel_is_refused_and_names_the_first() {
        let root = Scratch::new();
        let locks = root.path().join("locks");
        let first = acquire_in(
            &locks,
            &root.path().join("a"),
            &configuration(&["spaces/one"]),
        )
        .expect("first bridge");
        let error = acquire_in(
            &locks,
            &root.path().join("b"),
            &configuration(&["spaces/two", "spaces/one"]),
        )
        .expect_err("second bridge on spaces/one");
        let text = error.to_string();
        assert!(
            text.contains("channel spaces/one already has a live bridge"),
            "{text}"
        );
        assert!(
            text.contains(&format!("process {}", std::process::id())),
            "{text}"
        );
        assert!(
            text.contains(&root.path().join("a").display().to_string()),
            "{text}"
        );
        assert!(
            text.contains("agentctl chat repoint --bridge-state"),
            "{text}"
        );
        // The refused bridge holds nothing: spaces/two is free for a third.
        let third = acquire_in(
            &locks,
            &root.path().join("c"),
            &configuration(&["spaces/two"]),
        )
        .expect("spaces/two is free");
        drop((first, third));
        acquire_in(
            &locks,
            &root.path().join("b"),
            &configuration(&["spaces/one"]),
        )
        .expect("free once the first bridge exits");
    }

    #[test]
    fn the_same_state_directory_twice_says_so() {
        let root = Scratch::new();
        let locks = root.path().join("locks");
        let state = root.path().join("a");
        let _first = acquire_in(&locks, &state, &configuration(&["C1"])).expect("first");
        let text = acquire_in(&locks, &state, &configuration(&["C1"]))
            .expect_err("second")
            .to_string();
        assert!(
            text.contains("already serves this state directory"),
            "{text}"
        );
    }

    #[test]
    fn channels_on_different_plugins_do_not_collide() {
        let root = Scratch::new();
        let locks = root.path().join("locks");
        let _first =
            acquire_in(&locks, &root.path().join("a"), &configuration(&["C1"])).expect("first");
        let mut other = configuration(&["C1"]);
        other.subscription_plugin = "slack".to_owned();
        acquire_in(&locks, &root.path().join("b"), &other).expect("other plugin");
    }

    #[test]
    fn the_directory_comes_from_the_environment_or_the_user_id() {
        assert_eq!(
            lock_directory_from(Some(OsString::from("/x/locks"))),
            PathBuf::from("/x/locks")
        );
        assert_eq!(
            lock_directory_from(Some(OsString::new())),
            default_lock_directory()
        );
        assert_eq!(lock_directory_from(None), default_lock_directory());
        assert!(default_lock_directory().ends_with("chat-channels"));
    }
}
