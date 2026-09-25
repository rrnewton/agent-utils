//! Named, long-lived interactive agents sharing a Herdr workspace.
//!
//! Herdr owns terminals and harness processes. This layer owns durable names,
//! launch intent, queue routing, snapshots, and conservative tab teardown.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{
    DirBuilderExt, FileExt as UnixFileExt, MetadataExt, OpenOptionsExt, PermissionsExt,
};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agent::{self, AgentApi, AgentError, AgentRuntime, DrainOptions, QueueResult, Target};
use crate::client::{
    claude_active_screen, claude_prompt_is_exact_composer, claude_prompt_transcript_count,
    claude_staged_composer, muse_idle_composer, muse_startup_metadata, muse_trust_prompt,
    muse_verified_process_composer, muse_verified_process_idle_composer,
    muse_verified_process_prompt_in_composer, muse_verified_process_prompt_is_exact_composer,
    muse_verified_process_prompt_transcript_count, AgentPaneInfo, CustomLaunchObservation,
    CustomProcessIdentity, HerdrClient, Pane, PaneMove, PaneShellProof,
};

const BRACKETED_PASTE_START: &str = "\u{1b}[200~";
const BRACKETED_PASTE_END: &str = "\u{1b}[201~";
const MAX_AGENT_RECORD_BYTES: usize = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_QUEUE_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;
const HEALTH_SCHEMA: &str = "agentctl-health/v1";
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const SESSION_STORAGE_SCHEMA: &str = "agentctl-session/v3";
const LEGACY_SESSION_STORAGE_SCHEMA: &str = "agentctl-session/v2";
const LAUNCH_SPEC_SCHEMA: &str = "agentctl-launch/v2";
const LEGACY_LAUNCH_SPEC_SCHEMA: &str = "agentctl-launch/v1";
const GOAL_STATE_SCHEMA: &str = "agentctl-goal/v1";
const GOAL_TRANSACTION_SCHEMA: &str = "agentctl-goal-transaction/v1";
const GOAL_TRANSACTION_FILE: &str = "goal-transaction.json";
const MANAGED_DEAD_RETIREMENT_SCHEMA: &str = "agentctl-managed-dead-retirement/v1";
const MANAGED_DEAD_RETIREMENT_FILE: &str = "managed-dead-retirement.json";
const NATIVE_SESSION_SCHEMA: &str = "agentctl-native-session/v1";
const RELOCATION_SCHEMA: &str = "agentctl-relocation/v1";

fn rename_directory_noreplace_at(
    source_parent: &File,
    source_name: &str,
    destination_parent: &File,
    destination_name: &str,
) -> Result<()> {
    let source_bytes = CString::new(source_name.as_bytes())
        .map_err(|_| fail("agent archive source path contains NUL"))?;
    let destination_bytes = CString::new(destination_name.as_bytes())
        .map_err(|_| fail("agent archive destination path contains NUL"))?;
    let result = unsafe {
        libc::renameat2(
            source_parent.as_raw_fd(),
            source_bytes.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_bytes.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EEXIST) => Err(fail(format!(
            "refusing to replace existing agent archive {destination_name}",
        ))),
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) => Err(fail(
            "cannot archive agent: filesystem lacks atomic no-replace rename support",
        )),
        _ => Err(fail(format!(
            "cannot archive agent {source_name} as {destination_name}: {error}",
        ))),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InstalledArtifact {
    device: u64,
    inode: u64,
    size: u64,
    digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArtifactInstallState {
    NotInstalled,
    Installed(InstalledArtifact),
    Uncertain,
}

#[derive(Debug)]
struct ArtifactWriteError {
    error: Box<AgentError>,
    state: ArtifactInstallState,
}

impl std::fmt::Display for ArtifactWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl From<AgentError> for ArtifactWriteError {
    fn from(error: AgentError) -> Self {
        Self {
            error: Box::new(error),
            state: ArtifactInstallState::NotInstalled,
        }
    }
}

type ArtifactWriteResult = std::result::Result<InstalledArtifact, ArtifactWriteError>;

fn atomic_replace_bytes(
    pinned: &PinnedAgentDirectory,
    name: &str,
    content: &[u8],
) -> ArtifactWriteResult {
    atomic_replace_bytes_with(
        pinned,
        name,
        content,
        (
            |_pinned, _temporary| Ok(()),
            |_pinned, _name, _temporary| Ok(()),
        ),
    )
}

fn atomic_replace_bytes_with<AfterWrite, AfterRename>(
    pinned: &PinnedAgentDirectory,
    name: &str,
    content: &[u8],
    hooks: (AfterWrite, AfterRename),
) -> ArtifactWriteResult
where
    AfterWrite: FnOnce(&PinnedAgentDirectory, &str) -> Result<()>,
    AfterRename: FnOnce(&PinnedAgentDirectory, &str, &str) -> Result<()>,
{
    let (after_write, after_rename) = hooks;
    if !matches!(
        name,
        "agent.json" | "output.json" | MANAGED_DEAD_RETIREMENT_FILE
    ) {
        return Err(fail("unsupported pinned registry artifact name").into());
    }
    let limit = if matches!(name, "agent.json" | MANAGED_DEAD_RETIREMENT_FILE) {
        MAX_AGENT_RECORD_BYTES
    } else {
        MAX_SNAPSHOT_BYTES
    };
    if content.len() > limit {
        return Err(fail(format!("refusing {name} larger than {limit} bytes")).into());
    }
    let name_c = CString::new(name).map_err(|_| fail("registry artifact name contains NUL"))?;
    let mut selected = None;
    for attempt in 0_u32..128 {
        let candidate = format!(
            ".{name}-recovery-{}-{}-{attempt}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| fail(error.to_string()))?
                .as_nanos()
        );
        let candidate_c = CString::new(candidate.as_bytes())
            .map_err(|_| fail("registry staging name contains NUL"))?;
        let descriptor = unsafe {
            libc::openat(
                pinned.file.as_raw_fd(),
                candidate_c.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if descriptor >= 0 {
            selected = Some((candidate_c, unsafe { File::from_raw_fd(descriptor) }));
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(fail(format!("cannot stage output snapshot: {error}")).into());
        }
    }
    let (temporary, mut file) = selected.ok_or_else(|| {
        ArtifactWriteError::from(fail("cannot reserve private output snapshot staging file"))
    })?;
    let temporary_name = temporary
        .to_str()
        .expect("constructed registry staging name is UTF-8");
    let uid = unsafe { libc::getuid() };
    let mut created_identity = None;
    let mut rename_attempted = false;
    let mut renamed = false;
    let mut installed_verified = false;
    let write_result = (|| -> Result<()> {
        let created = file
            .metadata()
            .map_err(|error| fail(format!("cannot inspect registry staging file: {error}")))?;
        if !created.is_file()
            || created.uid() != uid
            || created.permissions().mode() & 0o077 != 0
            || created.nlink() != 1
        {
            return Err(fail("unsafe registry artifact staging file"));
        }
        let identity = (created.dev(), created.ino());
        created_identity = Some(identity);
        file.write_all(content)
            .map_err(|error| fail(format!("cannot write registry staging file: {error}")))?;
        file.sync_all()
            .map_err(|error| fail(format!("cannot sync registry staging file: {error}")))?;
        after_write(pinned, temporary_name)?;
        let held = file
            .metadata()
            .map_err(|error| fail(format!("cannot reinspect registry staging file: {error}")))?;
        let mut held_content = vec![0_u8; content.len() + 1];
        let held_length = file
            .read_at(&mut held_content, 0)
            .map_err(|error| fail(format!("cannot reread registry staging file: {error}")))?;
        let staged = ManagedAgents::<HerdrClient>::open_optional_pinned_file(
            pinned,
            temporary_name,
            libc::O_RDONLY,
        )?
        .ok_or_else(|| fail("registry artifact staging name disappeared before installation"))?;
        let staged_metadata = staged.metadata().map_err(|error| {
            fail(format!(
                "cannot inspect registry artifact staging name: {error}"
            ))
        })?;
        if !held.is_file()
            || !staged_metadata.is_file()
            || held.uid() != uid
            || staged_metadata.uid() != uid
            || held.permissions().mode() & 0o077 != 0
            || staged_metadata.permissions().mode() & 0o077 != 0
            || held.nlink() != 1
            || staged_metadata.nlink() != 1
            || held.len() != content.len() as u64
            || staged_metadata.len() != content.len() as u64
            || held_length != content.len()
            || &held_content[..held_length] != content
            || (held.dev(), held.ino()) != identity
            || (staged_metadata.dev(), staged_metadata.ino()) != identity
        {
            return Err(fail(format!(
                "registry artifact staging generation changed before installing {name}"
            )));
        }
        rename_attempted = true;
        let rename_result = unsafe {
            libc::renameat(
                pinned.file.as_raw_fd(),
                temporary.as_ptr(),
                pinned.file.as_raw_fd(),
                name_c.as_ptr(),
            )
        };
        if rename_result != 0 {
            return Err(fail(format!(
                "cannot install pinned registry artifact: {}",
                std::io::Error::last_os_error()
            )));
        }
        // The staging name no longer belongs to this operation. Never remove
        // a new entry that appears there after the rename.
        renamed = true;
        after_rename(pinned, name, temporary_name)?;
        let installed =
            ManagedAgents::<HerdrClient>::open_pinned_file(pinned, name, libc::O_RDONLY)?;
        let installed_metadata = installed.metadata().map_err(|error| {
            fail(format!(
                "cannot inspect installed registry artifact: {error}"
            ))
        })?;
        let held = file.metadata().map_err(|error| {
            fail(format!(
                "cannot reinspect installed registry artifact: {error}"
            ))
        })?;
        let mut held_content = vec![0_u8; content.len() + 1];
        let held_length = file.read_at(&mut held_content, 0).map_err(|error| {
            fail(format!(
                "cannot reread installed registry artifact: {error}"
            ))
        })?;
        if !installed_metadata.is_file()
            || installed_metadata.uid() != uid
            || installed_metadata.permissions().mode() & 0o077 != 0
            || installed_metadata.nlink() != 1
            || installed_metadata.len() != content.len() as u64
            || held_length != content.len()
            || &held_content[..held_length] != content
            || (installed_metadata.dev(), installed_metadata.ino()) != identity
            || (held.dev(), held.ino(), held.len())
                != (identity.0, identity.1, content.len() as u64)
        {
            return Err(fail(format!(
                "installed registry artifact {name} was not the staged generation"
            )));
        }
        installed_verified = true;
        pinned
            .file
            .sync_all()
            .map_err(|error| fail(format!("cannot sync installed registry artifact: {error}")))?;
        Ok(())
    })();
    let mut uncertain = renamed && !installed_verified;
    if !renamed && rename_attempted {
        let mut held_content = vec![0_u8; content.len() + 1];
        let held_content_exact = file
            .read_at(&mut held_content, 0)
            .is_ok_and(|length| length == content.len() && &held_content[..length] == content);
        let held_exact = file.metadata().is_ok_and(|metadata| {
            metadata.is_file()
                && metadata.uid() == uid
                && metadata.permissions().mode() & 0o077 == 0
                && metadata.nlink() == 1
                && metadata.len() == content.len() as u64
                && held_content_exact
                && Some((metadata.dev(), metadata.ino())) == created_identity
        });
        let staged_exact = ManagedAgents::<HerdrClient>::open_optional_pinned_file(
            pinned,
            temporary_name,
            libc::O_RDONLY,
        )
        .ok()
        .flatten()
        .and_then(|staged| staged.metadata().ok())
        .is_some_and(|metadata| {
            metadata.is_file()
                && metadata.uid() == uid
                && metadata.permissions().mode() & 0o077 == 0
                && metadata.nlink() == 1
                && Some((metadata.dev(), metadata.ino())) == created_identity
        });
        let installed_exact =
            ManagedAgents::<HerdrClient>::open_optional_pinned_file(pinned, name, libc::O_RDONLY)
                .ok()
                .flatten()
                .and_then(|installed| installed.metadata().ok())
                .is_some_and(|metadata| {
                    metadata.is_file()
                        && metadata.uid() == uid
                        && metadata.permissions().mode() & 0o077 == 0
                        && metadata.nlink() == 1
                        && metadata.len() == content.len() as u64
                        && Some((metadata.dev(), metadata.ino())) == created_identity
                });
        if held_exact && installed_exact && !staged_exact {
            renamed = true;
            installed_verified = true;
        } else if !(held_exact && staged_exact) {
            uncertain = true;
        }
    }
    let cleanup = if renamed || uncertain {
        Ok(())
    } else {
        match ManagedAgents::<HerdrClient>::open_optional_pinned_file(
        pinned,
        temporary_name,
        libc::O_RDONLY,
    ) {
        Ok(None) => Ok(()),
        Ok(Some(staged)) => match staged.metadata() {
            Ok(metadata)
                if metadata.is_file()
                    && metadata.uid() == uid
                    && metadata.permissions().mode() & 0o077 == 0
                    && metadata.nlink() == 1
                    && Some((metadata.dev(), metadata.ino())) == created_identity =>
            {
                let result = unsafe {
                    libc::unlinkat(pinned.file.as_raw_fd(), temporary.as_ptr(), 0)
                };
                if result == 0 {
                    Ok(())
                } else {
                    Err(fail(format!(
                        "cannot remove owned registry staging file: {}",
                        std::io::Error::last_os_error()
                    )))
                }
            }
            Ok(_) => Err(fail(
                "registry staging name no longer denotes the owned file; replacement was preserved",
            )),
            Err(error) => Err(fail(format!(
                "cannot inspect registry staging file during cleanup: {error}"
            ))),
        },
        Err(error) => Err(error),
        }
    };
    // A same-uid process ignoring the cooperative registry lock can still race
    // the final identity check above and unlinkat.
    let error = match (write_result, cleanup) {
        (Ok(()), Ok(())) => None,
        (Err(error), Ok(())) => Some(error),
        (Ok(()), Err(cleanup)) => Some(cleanup),
        (Err(error), Err(cleanup)) => Some(fail(format!(
            "{error}; registry artifact cleanup failed: {cleanup}"
        ))),
    };
    let artifact = created_identity.map(|(device, inode)| InstalledArtifact {
        device,
        inode,
        size: content.len() as u64,
        digest: Sha256::digest(content).into(),
    });
    match (error, artifact) {
        (None, Some(artifact)) if installed_verified => Ok(artifact),
        (None, _) => Err(ArtifactWriteError {
            error: Box::new(fail(format!(
                "cannot prove installed registry artifact {name}"
            ))),
            state: ArtifactInstallState::Uncertain,
        }),
        (Some(error), Some(artifact)) if installed_verified => Err(ArtifactWriteError {
            error: Box::new(error),
            state: ArtifactInstallState::Installed(artifact),
        }),
        (Some(error), _) if renamed || uncertain => Err(ArtifactWriteError {
            error: Box::new(error),
            state: ArtifactInstallState::Uncertain,
        }),
        (Some(error), _) => Err(ArtifactWriteError {
            error: Box::new(error),
            state: ArtifactInstallState::NotInstalled,
        }),
    }
}

/// Result of a managed-agent operation, including durable delivery outcomes.
pub type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

fn message_id(value: &str) -> bool {
    value.len() <= 255
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn name(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 32
        || value == "archive"
        || !value.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(fail(
            "agent name must start with a lowercase letter and contain 1-32 lowercase letters, digits or hyphens; 'archive' is reserved",
        ));
    }
    Ok(value)
}

/// Harness arguments for Codex/Claude/Muse presets, leaving permission policy untouched.
pub fn harness_arguments(
    harness: &str,
    model: Option<&str>,
    resume: Option<&str>,
    extra: &[String],
) -> Result<Vec<String>> {
    if harness.is_empty()
        || harness.len() > 64
        || !harness.as_bytes()[0].is_ascii_lowercase()
        || !harness
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(fail("harness must be a Herdr agent kind"));
    }
    if extra
        .iter()
        .any(|value| value.is_empty() || value.contains('\0'))
    {
        return Err(fail(
            "harness arguments must be nonempty and contain no NUL",
        ));
    }
    let mut arguments = Vec::new();
    match harness {
        "codex" => {
            if let Some(resume) = resume.filter(|value| !value.is_empty()) {
                arguments.extend(["resume".to_owned(), resume.to_owned()]);
            }
            arguments.push("--no-alt-screen".to_owned());
            if let Some(model) = model.filter(|value| !value.is_empty()) {
                arguments.extend(["--model".to_owned(), model.to_owned()]);
            }
        }
        "claude" => {
            if let Some(resume) = resume.filter(|value| !value.is_empty()) {
                arguments.extend(["--resume".to_owned(), resume.to_owned()]);
            }
            if let Some(model) = model.filter(|value| !value.is_empty()) {
                arguments.extend(["--model".to_owned(), model.to_owned()]);
            }
        }
        "muse" => {
            if resume.is_some() {
                return Err(fail(
                    "interactive Muse resume is not supported; use literal owner-configured argv",
                ));
            }
            if let Some(model) = model.filter(|value| !value.is_empty()) {
                arguments.extend(["--model".to_owned(), model.to_owned()]);
            }
        }
        _ if model.is_some() || resume.is_some() => return Err(fail(
            "model and resume presets support codex/claude/muse; use harness arguments for other kinds",
        )),
        _ => {}
    }
    if arguments.iter().any(|value| value.contains('\0')) {
        return Err(fail("harness arguments must contain no NUL"));
    }
    arguments.extend_from_slice(extra);
    Ok(arguments)
}

/// Validate literal `KEY=VALUE` entries for a newly created terminal.
pub fn environment_entries(values: &[String]) -> Result<Vec<String>> {
    let mut entries = Vec::with_capacity(values.len());
    for entry in values {
        let Some((name, value)) = entry.split_once('=') else {
            return Err(fail("environment entry must use KEY=VALUE"));
        };
        let valid_name = name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if name.contains('\0') || !valid_name {
            return Err(fail(
                "environment variable name must match [A-Za-z_][A-Za-z0-9_]*",
            ));
        }
        if value.contains('\0') {
            return Err(fail("environment variable value must contain no NUL"));
        }
        entries.push(entry.clone());
    }
    Ok(entries)
}

/// Lifecycle operations in addition to the existing interactive messaging API.
pub trait ManagedApi: AgentApi {
    /// Resolve a unique workspace label.
    fn workspace_id_for_label(&self, label: &str) -> crate::error::Result<Option<String>>;
    /// Create a workspace and return its workspace, tab, and pane IDs.
    fn create_workspace(
        &self,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> crate::error::Result<(String, String, String)>;
    /// Create a fresh labelled tab without stealing focus.
    fn create_tab(
        &self,
        workspace: &str,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> crate::error::Result<String>;
    /// Capture both ownership IDs in the same tab-allocation response.
    fn create_tab_with_pane(
        &self,
        workspace: &str,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> crate::error::Result<(String, String)>;
    /// Move one exact pane into a new tab in an existing workspace.
    fn move_pane_to_new_tab(
        &self,
        pane: &str,
        expected_terminal: &str,
        workspace: &str,
        label: &str,
    ) -> crate::error::Result<PaneMove>;
    /// Close exactly one owned pane, preserving any concurrently added siblings.
    fn close_pane(&self, pane: &str) -> crate::error::Result<()>;
    /// Focus an existing pane for direct human interaction.
    fn focus_pane(&self, pane: &str) -> crate::error::Result<()>;
    /// Rename one owned tab.
    fn rename_tab(&self, tab: &str, label: &str) -> crate::error::Result<()>;
    /// Start an interactive harness in a fresh shell pane.
    fn start_agent(
        &self,
        name: &str,
        harness: &str,
        pane: &str,
        args: &[String],
        timeout: Duration,
    ) -> crate::error::Result<()>;
    /// Start a custom harness through `pane run` and verify the foreground process.
    fn start_pane_agent(
        &self,
        _name: &str,
        _harness: &str,
        _pane: &str,
        _args: &[String],
        _timeout: Duration,
        _persist: &mut dyn FnMut(CustomLaunchObservation) -> crate::error::Result<()>,
    ) -> crate::error::Result<()> {
        Err(crate::error::AdapterError::unavailable(
            "custom pane harness launch is unavailable",
        ))
    }
    /// Recover or reconcile a custom launch by exact saved argv and live PID.
    fn recover_pane_agent(
        &self,
        _pane: &str,
        _expected_argv: &[String],
        _expected_device: u64,
        _expected_inode: u64,
        _expected_pid: u64,
    ) -> crate::error::Result<CustomProcessIdentity> {
        Err(crate::error::AdapterError::unavailable(
            "custom pane harness recovery is unavailable",
        ))
    }
    /// Commit recovered session state while its custom process stays pinned.
    fn commit_recovered_pane_agent(
        &self,
        pane: &str,
        harness: &str,
        identity: &CustomProcessIdentity,
        commit: &mut dyn FnMut() -> crate::error::Result<()>,
    ) -> crate::error::Result<()>;
    /// Require the configured custom harness in one exact foreground pane.
    fn verify_custom_harness(
        &self,
        _pane: &str,
        _harness: &str,
        _identity: Option<&CustomProcessIdentity>,
    ) -> crate::error::Result<()> {
        Err(crate::error::AdapterError::unavailable(
            "custom pane harness verification is unavailable",
        ))
    }
    /// Prove that one recorded custom-process generation is absent.
    fn process_generation_absent(
        &self,
        _expected: &CustomProcessIdentity,
    ) -> crate::error::Result<bool> {
        Err(crate::error::AdapterError::unavailable(
            "custom process generation proof is unavailable",
        ))
    }
    /// Prove the pane has returned to its original shell process group.
    fn pane_is_idle_shell(&self, _pane: &str) -> crate::error::Result<bool> {
        Err(crate::error::AdapterError::unavailable(
            "idle shell verification is unavailable",
        ))
    }
    /// Capture the exact process generation of the shell Herdr owns for a pane.
    fn pane_shell_identity(&self, _pane: &str) -> crate::error::Result<CustomProcessIdentity> {
        Err(crate::error::AdapterError::unavailable(
            "pane shell identity is unavailable",
        ))
    }
    /// Require the complete shell process generation captured at adoption.
    fn verify_pane_shell_identity(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
    ) -> crate::error::Result<()> {
        if self.pane_shell_identity(pane)? == *expected {
            Ok(())
        } else {
            Err(crate::error::AdapterError::unavailable(format!(
                "recorded pane shell generation changed for pane {pane}"
            )))
        }
    }
    /// Cancellation-aware complete adopted-shell generation check.
    fn verify_pane_shell_identity_with_runtime(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        if runtime.cancelled() {
            return Err(crate::error::AdapterError::unavailable(
                "Herdr control operation was cancelled",
            ));
        }
        self.verify_pane_shell_identity(pane, expected)
    }
    /// Prove both idle-shell state and the exact process generation captured earlier.
    fn pane_is_same_idle_shell(
        &self,
        _pane: &str,
        _expected: &CustomProcessIdentity,
    ) -> crate::error::Result<bool> {
        Err(crate::error::AdapterError::unavailable(
            "identity-bound idle shell verification is unavailable",
        ))
    }
    /// Capture one exact supported idle-shell generation with no descendants.
    fn pane_idle_shell_identity(
        &self,
        _pane: &str,
    ) -> crate::error::Result<Option<PaneShellProof>> {
        Err(crate::error::AdapterError::unavailable(
            "idle shell identity proof is unavailable",
        ))
    }
    /// Cancellation-aware exact idle-shell proof.
    fn pane_idle_shell_identity_with_runtime(
        &self,
        pane: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<Option<PaneShellProof>> {
        if runtime.cancelled() {
            return Err(crate::error::AdapterError::unavailable(
                "Herdr control operation was cancelled",
            ));
        }
        self.pane_idle_shell_identity(pane)
    }
    /// Cancellation-aware identity-bound idle-shell proof.
    fn pane_is_same_idle_shell_with_runtime(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<bool> {
        if runtime.cancelled() {
            return Err(crate::error::AdapterError::unavailable(
                "Herdr control operation was cancelled",
            ));
        }
        self.pane_is_same_idle_shell(pane, expected)
    }
    /// Cancellation-aware custom harness verification.
    fn verify_custom_harness_with_runtime(
        &self,
        pane: &str,
        harness: &str,
        identity: Option<&CustomProcessIdentity>,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        if runtime.cancelled() {
            return Err(crate::error::AdapterError::unavailable(
                "Herdr control operation was cancelled",
            ));
        }
        self.verify_custom_harness(pane, harness, identity)
    }
    /// Insert literal text without a submission key.
    fn send_text(&self, _pane: &str, _text: &str) -> crate::error::Result<()> {
        Err(crate::error::AdapterError::unavailable(
            "literal pane text insertion is unavailable",
        ))
    }
    /// Cancellation-aware literal text insertion.
    fn send_text_with_runtime(
        &self,
        pane: &str,
        text: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        if runtime.cancelled() {
            return Err(crate::error::AdapterError::unavailable(
                "Herdr control operation was cancelled",
            ));
        }
        self.send_text(pane, text)
    }
    /// Resolve an exact live Herdr agent name.
    fn agent_pane(&self, name: &str) -> crate::error::Result<String>;
    /// Resolve an exact live Herdr agent name with service cancellation.
    fn agent_pane_with_runtime(
        &self,
        name: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<String> {
        if runtime.cancelled() {
            return Err(crate::error::AdapterError::unavailable(
                "Herdr control operation was cancelled",
            ));
        }
        self.agent_pane(name)
    }
    /// Report the explicitly supplied native session identity.
    fn report_agent_session(
        &self,
        name: &str,
        pane: &str,
        kind: &str,
        session: &str,
    ) -> crate::error::Result<()>;
    /// Send one explicit key to the named pane.
    fn send_keys(&self, pane: &str, key: &str) -> crate::error::Result<()>;
    /// Send one key with service cancellation.
    fn send_keys_with_runtime(
        &self,
        pane: &str,
        key: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        if runtime.cancelled() {
            return Err(crate::error::AdapterError::unavailable(
                "Herdr control operation was cancelled",
            ));
        }
        self.send_keys(pane, key)
    }
    /// Close one owned tab; never close its workspace.
    fn close_tab(&self, tab: &str) -> crate::error::Result<()>;
}

impl ManagedApi for HerdrClient {
    fn agent_pane(&self, name: &str) -> crate::error::Result<String> {
        HerdrClient::agent_pane(self, name)
    }
    fn agent_pane_with_runtime(
        &self,
        name: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<String> {
        HerdrClient::agent_pane_with_cancellation(self, name, &|| runtime.cancelled())
    }
    fn report_agent_session(
        &self,
        name: &str,
        pane: &str,
        kind: &str,
        session: &str,
    ) -> crate::error::Result<()> {
        HerdrClient::report_agent_session(self, name, pane, kind, session)
    }
    fn send_keys(&self, pane: &str, key: &str) -> crate::error::Result<()> {
        HerdrClient::send_keys(self, pane, key)
    }
    fn send_keys_with_runtime(
        &self,
        pane: &str,
        key: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        HerdrClient::send_keys_with_cancellation(self, pane, key, &|| runtime.cancelled())
    }

    fn workspace_id_for_label(&self, label: &str) -> crate::error::Result<Option<String>> {
        HerdrClient::workspace_id_for_label(self, label)
    }
    fn create_workspace(
        &self,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> crate::error::Result<(String, String, String)> {
        HerdrClient::create_workspace(self, label, cwd, environment)
    }
    fn create_tab(
        &self,
        workspace: &str,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> crate::error::Result<String> {
        HerdrClient::create_tab(self, workspace, label, cwd, environment)
    }
    fn create_tab_with_pane(
        &self,
        workspace: &str,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> crate::error::Result<(String, String)> {
        HerdrClient::create_tab_with_pane(self, workspace, label, cwd, environment)
    }
    fn move_pane_to_new_tab(
        &self,
        pane: &str,
        expected_terminal: &str,
        workspace: &str,
        label: &str,
    ) -> crate::error::Result<PaneMove> {
        HerdrClient::move_pane_to_new_tab(self, pane, expected_terminal, workspace, label)
    }
    fn close_pane(&self, pane: &str) -> crate::error::Result<()> {
        HerdrClient::close_pane(self, pane)
    }
    fn focus_pane(&self, pane: &str) -> crate::error::Result<()> {
        HerdrClient::focus_pane(self, pane)
    }
    fn rename_tab(&self, tab: &str, label: &str) -> crate::error::Result<()> {
        HerdrClient::rename_tab(self, tab, label)
    }
    fn start_agent(
        &self,
        name: &str,
        harness: &str,
        pane: &str,
        args: &[String],
        timeout: Duration,
    ) -> crate::error::Result<()> {
        HerdrClient::start_agent(self, name, harness, pane, args, timeout)
    }
    fn start_pane_agent(
        &self,
        name: &str,
        harness: &str,
        pane: &str,
        args: &[String],
        timeout: Duration,
        persist: &mut dyn FnMut(CustomLaunchObservation) -> crate::error::Result<()>,
    ) -> crate::error::Result<()> {
        HerdrClient::start_pane_agent(self, name, harness, pane, args, timeout, persist)
    }
    fn recover_pane_agent(
        &self,
        pane: &str,
        expected_argv: &[String],
        expected_device: u64,
        expected_inode: u64,
        expected_pid: u64,
    ) -> crate::error::Result<CustomProcessIdentity> {
        HerdrClient::recover_pane_agent(
            self,
            pane,
            expected_argv,
            expected_device,
            expected_inode,
            expected_pid,
        )
    }
    fn commit_recovered_pane_agent(
        &self,
        pane: &str,
        harness: &str,
        identity: &CustomProcessIdentity,
        commit: &mut dyn FnMut() -> crate::error::Result<()>,
    ) -> crate::error::Result<()> {
        HerdrClient::commit_recovered_pane_agent(self, pane, harness, identity, commit)
    }
    fn verify_custom_harness(
        &self,
        pane: &str,
        harness: &str,
        identity: Option<&CustomProcessIdentity>,
    ) -> crate::error::Result<()> {
        HerdrClient::verify_custom_harness(self, pane, harness, identity)
    }
    fn process_generation_absent(
        &self,
        expected: &CustomProcessIdentity,
    ) -> crate::error::Result<bool> {
        HerdrClient::process_generation_absent(self, expected)
    }
    fn pane_is_idle_shell(&self, pane: &str) -> crate::error::Result<bool> {
        HerdrClient::pane_is_idle_shell(self, pane)
    }
    fn pane_shell_identity(&self, pane: &str) -> crate::error::Result<CustomProcessIdentity> {
        HerdrClient::pane_shell_identity(self, pane)
    }
    fn verify_pane_shell_identity(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
    ) -> crate::error::Result<()> {
        HerdrClient::verify_pane_shell_identity(self, pane, expected)
    }
    fn verify_pane_shell_identity_with_runtime(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        HerdrClient::verify_pane_shell_identity_with_cancellation(self, pane, expected, &|| {
            runtime.cancelled()
        })
    }
    fn pane_is_same_idle_shell(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
    ) -> crate::error::Result<bool> {
        HerdrClient::pane_is_same_idle_shell(self, pane, expected)
    }
    fn pane_idle_shell_identity(&self, pane: &str) -> crate::error::Result<Option<PaneShellProof>> {
        HerdrClient::pane_idle_shell_identity(self, pane)
    }
    fn pane_idle_shell_identity_with_runtime(
        &self,
        pane: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<Option<PaneShellProof>> {
        HerdrClient::pane_idle_shell_identity_with_cancellation(self, pane, &|| runtime.cancelled())
    }
    fn pane_is_same_idle_shell_with_runtime(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<bool> {
        HerdrClient::pane_is_same_idle_shell_with_cancellation(self, pane, expected, &|| {
            runtime.cancelled()
        })
    }
    fn verify_custom_harness_with_runtime(
        &self,
        pane: &str,
        harness: &str,
        identity: Option<&CustomProcessIdentity>,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        HerdrClient::verify_custom_harness_with_cancellation(self, pane, harness, identity, &|| {
            runtime.cancelled()
        })
    }
    fn send_text(&self, pane: &str, text: &str) -> crate::error::Result<()> {
        HerdrClient::send_text(self, pane, text)
    }
    fn send_text_with_runtime(
        &self,
        pane: &str,
        text: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        HerdrClient::send_text_with_cancellation(self, pane, text, &|| runtime.cancelled())
    }
    fn close_tab(&self, tab: &str) -> crate::error::Result<()> {
        HerdrClient::close_tab(self, tab)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct LegacyAgentRecord {
    #[serde(default = "herdr_adapter")]
    adapter: String,
    #[serde(default = "interactive_mode")]
    mode: String,
    #[serde(default = "herdr_adapter")]
    backend: String,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    runtime_home: Option<String>,
    #[serde(default)]
    pane_reported_by_agentctl: bool,
    #[serde(default)]
    custom_process_identity: Option<CustomProcessIdentity>,
    #[serde(default)]
    foreign_shell_identity: Option<CustomProcessIdentity>,
    #[serde(default)]
    launch_profile: Option<String>,
    #[serde(default)]
    launch_executable: Option<String>,
    #[serde(default)]
    launch_executable_device: Option<u64>,
    #[serde(default)]
    launch_executable_inode: Option<u64>,
    #[serde(default)]
    launch_argv: Vec<String>,
    #[serde(default)]
    launch_environment_names: Vec<String>,
    #[serde(default)]
    launch_permission_mode: Option<String>,
    #[serde(default)]
    runtime_ownership: Option<String>,
    #[serde(default)]
    runner_pid: Option<u64>,
    #[serde(default)]
    runner_started_at: Option<String>,
    #[serde(default)]
    runner_identity: Option<CustomProcessIdentity>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
    name: String,
    token: String,
    harness: String,
    cwd: String,
    created_at: f64,
    schema: u32,
    lifecycle: String,
    workspace_id: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
    session_agent: Option<String>,
    session_value: Option<String>,
    #[serde(default)]
    session_source: Option<String>,
    model: Option<String>,
    resume: Option<String>,
    #[serde(default)]
    startup_warning: Option<String>,
    #[serde(default)]
    effective_reasoning_effort: Option<String>,
    error: Option<String>,
    goal: Option<String>,
    goal_delivery: Option<String>,
    goal_session_id: Option<String>,
    goal_command: Option<Vec<String>>,
    #[serde(default)]
    goal_messages: BTreeMap<String, String>,
    goal_message_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchExecutable {
    path: String,
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchSpec {
    schema: String,
    harness: String,
    cwd: String,
    adapter: String,
    mode: String,
    backend: String,
    model: Option<String>,
    resume: Option<String>,
    profile: Option<String>,
    argv: Vec<String>,
    environment_names: Vec<String>,
    runtime_home: Option<String>,
    runtime_ownership: String,
    executable: Option<LaunchExecutable>,
    #[serde(default)]
    permission_mode: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct AgentRecord {
    name: String,
    token: String,
    launch: LaunchSpec,
    created_at: f64,
    lifecycle: String,
    workspace_id: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
    session_agent: Option<String>,
    session_value: Option<String>,
    session_source: Option<String>,
    startup_warning: Option<String>,
    effective_reasoning_effort: Option<String>,
    error: Option<String>,
    goal: Option<String>,
    goal_command: Option<Vec<String>>,
    goal_message_id: Option<String>,
    paused: bool,
    pane_reported_by_agentctl: bool,
    custom_process_identity: Option<CustomProcessIdentity>,
    foreign_shell_identity: Option<CustomProcessIdentity>,
    runner_identity: Option<CustomProcessIdentity>,
    legacy_runner_pid: Option<u64>,
    legacy_runner_started_at: Option<String>,
    legacy_goal_delivery: Option<String>,
    legacy_goal_messages: BTreeMap<String, String>,
    legacy_goal_pointer: bool,
    extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeadPaneProof {
    info: AgentPaneInfo,
    presentation: Pane,
    shell: PaneShellProof,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GoalArtifactState {
    Prepared,
    Pending,
    Inflight,
    Processed,
    Failed,
}

#[derive(Clone, Debug)]
struct LegacyRecordSnapshot {
    record: AgentRecord,
    content: Vec<u8>,
    digest: String,
    directory_device: u64,
    directory_inode: u64,
}

#[derive(Clone, Debug)]
struct ManagedRecordSnapshot {
    record: AgentRecord,
    content: Vec<u8>,
    directory_device: u64,
    directory_inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneRoute {
    workspace_id: String,
    tab_id: String,
    pane_id: String,
}

impl PaneRoute {
    fn from_pane(pane: &Pane) -> Self {
        Self {
            workspace_id: pane.workspace_id.clone(),
            tab_id: pane.tab_id.clone(),
            pane_id: pane.pane_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelocationJournal {
    schema: String,
    token: String,
    terminal_id: String,
    old: PaneRoute,
    target_workspace_id: String,
    new_tab: bool,
}

impl RelocationJournal {
    fn validate(&self, token: &str) -> Result<()> {
        let valid = |value: &str| !value.is_empty() && !value.contains('\0');
        if self.schema != RELOCATION_SCHEMA
            || self.token != token
            || !self.new_tab
            || !valid(&self.terminal_id)
            || !valid(&self.target_workspace_id)
            || !valid(&self.old.workspace_id)
            || !valid(&self.old.tab_id)
            || !valid(&self.old.pane_id)
        {
            return Err(fail("invalid relocation journal"));
        }
        Ok(())
    }
}

impl ManagedRecordSnapshot {
    fn same_generation(&self, other: &Self) -> bool {
        self.record == other.record
            && self.content == other.content
            && self.directory_device == other.directory_device
            && self.directory_inode == other.directory_inode
    }
}

#[derive(Debug)]
struct PinnedAgentDirectory {
    name: String,
    path: PathBuf,
    file: File,
    device: u64,
    inode: u64,
}

#[derive(Debug)]
struct PinnedParentDirectory {
    path: PathBuf,
    file: File,
    device: u64,
    inode: u64,
}

impl LegacyRecordSnapshot {
    fn same_generation(&self, other: &Self) -> bool {
        self.content == other.content
            && self.digest == other.digest
            && self.directory_device == other.directory_device
            && self.directory_inode == other.directory_inode
    }
}

fn herdr_adapter() -> String {
    "herdr".to_owned()
}
fn interactive_mode() -> String {
    "interactive".to_owned()
}

impl LegacyAgentRecord {
    fn launch_spec(&self) -> Result<LaunchSpec> {
        let runtime_ownership = self.runtime_ownership.clone().unwrap_or_else(|| {
            if self.adapter == "herdr-foreign" {
                "foreign".to_owned()
            } else {
                "owned".to_owned()
            }
        });
        let mut argv = self.launch_argv.clone();
        if argv.is_empty() && self.adapter != "herdr-foreign" {
            argv.push(self.harness.clone());
        }
        let executable = match (
            self.launch_executable.as_ref(),
            self.launch_executable_device,
            self.launch_executable_inode,
        ) {
            (None, None, None) => None,
            (Some(path), Some(device), Some(inode)) => Some(LaunchExecutable {
                path: path.clone(),
                device,
                inode,
            }),
            _ => return Err(fail("incomplete launch executable identity")),
        };
        Ok(LaunchSpec {
            schema: LAUNCH_SPEC_SCHEMA.to_owned(),
            harness: self.harness.clone(),
            cwd: self.cwd.clone(),
            adapter: self.adapter.clone(),
            mode: self.mode.clone(),
            backend: self.backend.clone(),
            model: self.model.clone(),
            resume: self.resume.clone(),
            profile: self.launch_profile.clone(),
            argv,
            environment_names: self.launch_environment_names.clone(),
            runtime_home: self.runtime_home.clone(),
            runtime_ownership,
            executable,
            permission_mode: self.launch_permission_mode.clone(),
        })
    }
}

impl AgentRecord {
    fn arguments(&self) -> &[String] {
        self.launch.argv.get(1..).unwrap_or(&[])
    }

    fn capabilities(&self) -> Vec<&'static str> {
        if self.launch.adapter == "turn-runner" {
            return vec!["status"];
        }
        let mut capabilities = vec![
            "send",
            "status",
            "read",
            "wait",
            "stop",
            "attach",
            "pause",
            "resume",
            "terminal-snapshot",
            "drain",
            "goal",
            "bind-session",
            "relocate",
        ];
        if self.launch.adapter == "herdr-pane" && self.launch.harness == "muse" {
            capabilities.push("reconcile-delivery");
        }
        capabilities
    }

    fn public_value(&self) -> Value {
        let executable = self.launch.executable.as_ref();
        let runner_pid = self
            .runner_identity
            .as_ref()
            .map_or(self.legacy_runner_pid, |identity| Some(identity.pid));
        let runner_started_at = self.runner_identity.as_ref().map_or_else(
            || self.legacy_runner_started_at.clone(),
            |identity| Some(identity.starttime_ticks.to_string()),
        );
        let mut value = json!({
            "schema": 1,
            "name": self.name,
            "token": self.token,
            "harness": self.launch.harness,
            "cwd": self.launch.cwd,
            "created_at": self.created_at,
            "lifecycle": self.lifecycle,
            "workspace_id": self.workspace_id,
            "tab_id": self.tab_id,
            "pane_id": self.pane_id,
            "session_agent": self.session_agent,
            "session_value": self.session_value,
            "model": self.launch.model,
            "resume": self.launch.resume,
            "startup_warning": self.startup_warning,
            "effective_reasoning_effort": self.effective_reasoning_effort,
            "error": self.error,
            "goal": self.goal,
            "goal_delivery": self.legacy_goal_delivery,
            "goal_session_id": self.session_value,
            "goal_command": self.goal_command,
            "goal_messages": {},
            "goal_message_id": self.goal_message_id,
            "adapter": self.launch.adapter,
            "mode": self.launch.mode,
            "backend": self.launch.backend,
            "paused": self.paused,
            "runtime_home": self.launch.runtime_home,
            "pane_reported_by_agentctl": self.pane_reported_by_agentctl,
            "custom_process_identity": self.custom_process_identity,
            "foreign_shell_identity": self.foreign_shell_identity,
            "launch_profile": self.launch.profile,
            "launch_executable": executable.map(|value| &value.path),
            "launch_executable_device": executable.map(|value| value.device),
            "launch_executable_inode": executable.map(|value| value.inode),
            "launch_argv": self.launch.argv,
            "launch_environment_names": self.launch.environment_names,
            "runtime_ownership": self.launch.runtime_ownership,
            "runner_pid": runner_pid,
            "runner_started_at": runner_started_at,
            "runner_identity": self.runner_identity,
        });
        value["arguments"] = json!(self.arguments());
        let object = value
            .as_object_mut()
            .expect("public session projection is an object");
        object.insert(
            "launch_permission_mode".to_owned(),
            json!(self.launch.permission_mode),
        );
        object.insert("session_source".to_owned(), json!(self.session_source));
        for (key, extension) in &self.extra {
            assert!(!object.contains_key(key), "validated extension collision");
            object.insert(key.clone(), extension.clone());
        }
        value
    }

    fn storage_value(&self) -> Result<Value> {
        if (self.session_value.is_some() != self.session_agent.is_some())
            || (self.session_value.is_some() != self.session_source.is_some())
            || self
                .session_agent
                .as_ref()
                .is_some_and(|agent| agent != &self.launch.harness)
        {
            return Err(fail("incomplete native session identity"));
        }
        self.validate_loaded(Path::new("<in-memory>"), &self.name)?;
        let launch = &self.launch;
        if self.runner_identity.is_none()
            && (self.legacy_runner_pid.is_some() || self.legacy_runner_started_at.is_some())
        {
            return Err(fail(
                "cannot migrate a PID/start-only runner without boot-bound identity",
            ));
        }
        self.validate_runtime_shape(launch)?;
        let native_session = self.session_value.as_ref().map(|value| {
            json!({
                "schema": NATIVE_SESSION_SCHEMA,
                "agent": self.session_agent.as_deref().unwrap_or(&launch.harness),
                "value": value,
                "source": self.session_source.as_deref().unwrap_or("observed"),
            })
        });
        Ok(json!({
            "schema": SESSION_STORAGE_SCHEMA,
            "name": self.name,
            "token": self.token,
            "created_at": self.created_at,
            "lifecycle": self.lifecycle,
            "launch": launch,
            "workspace_id": self.workspace_id,
            "tab_id": self.tab_id,
            "pane_id": self.pane_id,
            "native_session": native_session,
            "startup_warning": self.startup_warning,
            "effective_reasoning_effort": self.effective_reasoning_effort,
            "error": self.error,
            "goal": {
                "schema": GOAL_STATE_SCHEMA,
                "objective": self.goal,
                "message_id": self.goal_message_id,
                "native_command": self.goal_command,
            },
            "paused": self.paused,
            "pane_reported_by_agentctl": self.pane_reported_by_agentctl,
            "custom_process_identity": self.custom_process_identity,
            "foreign_shell_identity": self.foreign_shell_identity,
            "runner_identity": self.runner_identity,
            "extensions": self.extra,
        }))
    }

    fn from_storage_value(value: Value, path: &Path, agent_name: &str) -> Result<Self> {
        let mut document = value
            .as_object()
            .cloned()
            .ok_or_else(|| fail(format!("invalid agent record: {}", path.display())))?;
        let legacy_goal_pointer = match document.get("schema") {
            Some(Value::String(schema))
                if schema == SESSION_STORAGE_SCHEMA || schema == LEGACY_SESSION_STORAGE_SCHEMA =>
            {
                const CURRENT_FIELDS: [&str; 19] = [
                    "schema",
                    "name",
                    "token",
                    "created_at",
                    "lifecycle",
                    "launch",
                    "workspace_id",
                    "tab_id",
                    "pane_id",
                    "native_session",
                    "startup_warning",
                    "effective_reasoning_effort",
                    "error",
                    "goal",
                    "paused",
                    "pane_reported_by_agentctl",
                    "custom_process_identity",
                    "foreign_shell_identity",
                    "runner_identity",
                ];
                const LEGACY_FIELDS: [&str; 25] = [
                    "schema",
                    "name",
                    "token",
                    "created_at",
                    "lifecycle",
                    "launch",
                    "workspace_id",
                    "tab_id",
                    "pane_id",
                    "session_agent",
                    "session_value",
                    "startup_warning",
                    "effective_reasoning_effort",
                    "error",
                    "goal",
                    "goal_delivery",
                    "goal_session_id",
                    "goal_command",
                    "goal_messages",
                    "goal_message_id",
                    "paused",
                    "pane_reported_by_agentctl",
                    "custom_process_identity",
                    "foreign_shell_identity",
                    "runner_identity",
                ];
                let current = schema == SESSION_STORAGE_SCHEMA;
                let fields: &[&str] = if current {
                    &CURRENT_FIELDS
                } else {
                    &LEGACY_FIELDS
                };
                if document.len() != fields.len() + 1
                    || !fields.iter().all(|field| document.contains_key(*field))
                    || !document.contains_key("extensions")
                {
                    return Err(fail(format!(
                        "invalid agent record {schema} fields: {}",
                        path.display()
                    )));
                }
                if current {
                    let goal = document
                        .get("goal")
                        .and_then(Value::as_object)
                        .cloned()
                        .ok_or_else(|| {
                            fail(format!(
                                "invalid agent record goal state: {}",
                                path.display()
                            ))
                        })?;
                    const GOAL_FIELDS: [&str; 4] =
                        ["schema", "objective", "message_id", "native_command"];
                    if goal.len() != GOAL_FIELDS.len()
                        || !GOAL_FIELDS.iter().all(|field| goal.contains_key(*field))
                        || goal.get("schema").and_then(Value::as_str) != Some(GOAL_STATE_SCHEMA)
                    {
                        return Err(fail(format!(
                            "invalid agent record goal state: {}",
                            path.display()
                        )));
                    }
                    document.insert(
                        "goal".to_owned(),
                        goal.get("objective").cloned().unwrap_or(Value::Null),
                    );
                    document.insert("goal_delivery".to_owned(), Value::Null);
                    document.insert(
                        "goal_command".to_owned(),
                        goal.get("native_command").cloned().unwrap_or(Value::Null),
                    );
                    document.insert("goal_messages".to_owned(), json!({}));
                    document.insert(
                        "goal_message_id".to_owned(),
                        goal.get("message_id").cloned().unwrap_or(Value::Null),
                    );
                    let native = document.remove("native_session");
                    match native {
                        Some(Value::Null) => {
                            document.insert("session_agent".to_owned(), Value::Null);
                            document.insert("session_value".to_owned(), Value::Null);
                            document.insert("session_source".to_owned(), Value::Null);
                            document.insert("goal_session_id".to_owned(), Value::Null);
                        }
                        Some(Value::Object(native)) => {
                            const SESSION_FIELDS: [&str; 4] =
                                ["schema", "agent", "value", "source"];
                            if native.len() != SESSION_FIELDS.len()
                                || !SESSION_FIELDS
                                    .iter()
                                    .all(|field| native.contains_key(*field))
                                || native.get("schema").and_then(Value::as_str)
                                    != Some(NATIVE_SESSION_SCHEMA)
                            {
                                return Err(fail(format!(
                                    "invalid agent record native session: {}",
                                    path.display()
                                )));
                            }
                            document.insert(
                                "session_agent".to_owned(),
                                native.get("agent").cloned().unwrap_or(Value::Null),
                            );
                            document.insert(
                                "session_value".to_owned(),
                                native.get("value").cloned().unwrap_or(Value::Null),
                            );
                            document.insert(
                                "session_source".to_owned(),
                                native.get("source").cloned().unwrap_or(Value::Null),
                            );
                            document.insert(
                                "goal_session_id".to_owned(),
                                native.get("value").cloned().unwrap_or(Value::Null),
                            );
                        }
                        _ => {
                            return Err(fail(format!(
                                "invalid agent record native session: {}",
                                path.display()
                            )))
                        }
                    }
                }
                let launch_value = document.remove("launch").expect("checked launch field");
                let launch_fields = launch_value.as_object().ok_or_else(|| {
                    fail(format!(
                        "invalid agent record launch specification: {}",
                        path.display()
                    ))
                })?;
                let launch_field_count = launch_fields.len();
                let has_permission_mode = launch_fields.contains_key("permission_mode");
                let mut launch: LaunchSpec =
                    serde_json::from_value(launch_value).map_err(|error| {
                        fail(format!(
                            "invalid agent record launch specification {}: {error}",
                            path.display()
                        ))
                    })?;
                let valid_launch_shape = match launch.schema.as_str() {
                    LAUNCH_SPEC_SCHEMA => launch_field_count == 15 && has_permission_mode,
                    LEGACY_LAUNCH_SPEC_SCHEMA => launch_field_count == 14 && !has_permission_mode,
                    _ => false,
                };
                if !valid_launch_shape {
                    return Err(fail(format!(
                        "invalid agent record launch schema: {}",
                        path.display()
                    )));
                }
                launch.schema = LAUNCH_SPEC_SCHEMA.to_owned();
                let extensions = document
                    .remove("extensions")
                    .and_then(|value| value.as_object().cloned())
                    .ok_or_else(|| {
                        fail(format!(
                            "invalid agent record extensions: {}",
                            path.display()
                        ))
                    })?;
                const LAUNCH_FIELDS: [&str; 15] = [
                    "schema",
                    "harness",
                    "cwd",
                    "adapter",
                    "mode",
                    "backend",
                    "model",
                    "resume",
                    "profile",
                    "argv",
                    "environment_names",
                    "runtime_home",
                    "runtime_ownership",
                    "executable",
                    "permission_mode",
                ];
                document.insert("schema".to_owned(), json!(1));
                document.insert("harness".to_owned(), json!(launch.harness));
                document.insert("cwd".to_owned(), json!(launch.cwd));
                document.insert("adapter".to_owned(), json!(launch.adapter));
                document.insert("mode".to_owned(), json!(launch.mode));
                document.insert("backend".to_owned(), json!(launch.backend));
                document.insert("model".to_owned(), json!(launch.model));
                document.insert("resume".to_owned(), json!(launch.resume));
                document.insert("launch_profile".to_owned(), json!(launch.profile));
                document.insert("launch_argv".to_owned(), json!(launch.argv));
                document.insert(
                    "launch_environment_names".to_owned(),
                    json!(launch.environment_names),
                );
                document.insert("runtime_home".to_owned(), json!(launch.runtime_home));
                document.insert(
                    "runtime_ownership".to_owned(),
                    json!(launch.runtime_ownership),
                );
                let (path_value, device, inode) =
                    launch
                        .executable
                        .map_or((Value::Null, Value::Null, Value::Null), |identity| {
                            (
                                json!(identity.path),
                                json!(identity.device),
                                json!(identity.inode),
                            )
                        });
                document.insert("launch_executable".to_owned(), path_value);
                document.insert("launch_executable_device".to_owned(), device);
                document.insert("launch_executable_inode".to_owned(), inode);
                document.insert(
                    "launch_permission_mode".to_owned(),
                    json!(launch.permission_mode),
                );
                for (key, value) in extensions {
                    if key == "extensions"
                        || fields.contains(&key.as_str())
                        || document.contains_key(&key)
                        || LAUNCH_FIELDS.contains(&key.as_str())
                        || matches!(
                            key.as_str(),
                            "arguments"
                                | "launch_profile"
                                | "launch_argv"
                                | "launch_environment_names"
                                | "launch_executable"
                                | "launch_executable_device"
                                | "launch_executable_inode"
                                | "launch_permission_mode"
                                | "runner_pid"
                                | "runner_started_at"
                        )
                    {
                        return Err(fail(format!(
                            "invalid agent record extension {:?}: {}",
                            key,
                            path.display()
                        )));
                    }
                    document.insert(key, value);
                }
                if !current {
                    let observed = document
                        .get("session_value")
                        .cloned()
                        .unwrap_or(Value::Null);
                    let asserted = document
                        .get("goal_session_id")
                        .cloned()
                        .unwrap_or(Value::Null);
                    if !observed.is_null() && !asserted.is_null() && observed != asserted {
                        return Err(fail(format!(
                            "contradictory native session identities in {}",
                            path.display()
                        )));
                    }
                    let (value, source) = if !observed.is_null() {
                        (observed, json!("observed"))
                    } else if !asserted.is_null() {
                        (asserted, json!("asserted"))
                    } else {
                        (Value::Null, Value::Null)
                    };
                    if !value.is_null() && document.get("session_agent").is_none_or(Value::is_null)
                    {
                        document.insert(
                            "session_agent".to_owned(),
                            document.get("harness").cloned().unwrap_or(Value::Null),
                        );
                    }
                    document.insert("session_value".to_owned(), value.clone());
                    document.insert("goal_session_id".to_owned(), value);
                    document.insert("session_source".to_owned(), source);
                }
                !current
            }
            Some(Value::Number(schema)) if schema.as_u64() == Some(1) => {
                if !document.contains_key("arguments") {
                    return Err(fail(format!("invalid agent record: {}", path.display())));
                }
                if !document.contains_key("runtime_ownership")
                    || document["runtime_ownership"].is_null()
                {
                    let ownership = if document
                        .get("adapter")
                        .and_then(Value::as_str)
                        .unwrap_or("herdr")
                        == "herdr-foreign"
                    {
                        "foreign"
                    } else {
                        "owned"
                    };
                    document.insert("runtime_ownership".to_owned(), json!(ownership));
                }
                let observed = document
                    .get("session_value")
                    .cloned()
                    .unwrap_or(Value::Null);
                let asserted = document
                    .get("goal_session_id")
                    .cloned()
                    .unwrap_or(Value::Null);
                if !observed.is_null() && !asserted.is_null() && observed != asserted {
                    return Err(fail(format!(
                        "contradictory native session identities in {}",
                        path.display()
                    )));
                }
                let (value, source) = if !observed.is_null() {
                    (observed, json!("observed"))
                } else if !asserted.is_null() {
                    (asserted, json!("asserted"))
                } else {
                    (Value::Null, Value::Null)
                };
                if !value.is_null() && document.get("session_agent").is_none_or(Value::is_null) {
                    document.insert(
                        "session_agent".to_owned(),
                        document.get("harness").cloned().unwrap_or(Value::Null),
                    );
                }
                document.insert("session_value".to_owned(), value.clone());
                document.insert("goal_session_id".to_owned(), value);
                document.insert("session_source".to_owned(), source);
                true
            }
            _ => {
                return Err(fail(format!(
                    "invalid agent record schema: {}",
                    path.display()
                )))
            }
        };
        let raw_arguments = document.remove("arguments").unwrap_or_else(|| json!([]));
        let arguments = raw_arguments.as_array().ok_or_else(|| {
            fail(format!(
                "invalid agent record arguments: {}",
                path.display()
            ))
        })?;
        if arguments.iter().any(|value| {
            value
                .as_str()
                .is_none_or(|argument| argument.contains('\0'))
        }) {
            return Err(fail(format!(
                "invalid agent record arguments: {}",
                path.display()
            )));
        }
        let arguments = arguments
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let launch_argv = document
            .get("launch_argv")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if launch_argv.is_empty() && !arguments.is_empty() {
            let program = document
                .get("launch_executable")
                .and_then(Value::as_str)
                .or_else(|| document.get("harness").and_then(Value::as_str))
                .ok_or_else(|| fail(format!("invalid agent record: {}", path.display())))?;
            document.insert(
                "launch_argv".to_owned(),
                json!(std::iter::once(program.to_owned())
                    .chain(arguments.iter().cloned())
                    .collect::<Vec<_>>()),
            );
        } else if !arguments.is_empty()
            && (launch_argv.len() != arguments.len() + 1
                || launch_argv[1..]
                    .iter()
                    .zip(&arguments)
                    .any(|(stored, expected)| stored.as_str() != Some(expected.as_str())))
        {
            return Err(fail(format!(
                "contradictory launch arguments in {}",
                path.display()
            )));
        }
        let legacy: LegacyAgentRecord = serde_json::from_value(Value::Object(document))
            .map_err(|error| fail(format!("invalid agent record {}: {error}", path.display())))?;
        if legacy
            .extra
            .keys()
            .any(|key| matches!(key.as_str(), "launch" | "native_session" | "extensions"))
        {
            return Err(fail(format!(
                "legacy agent record contains a reserved current-schema field: {}",
                path.display()
            )));
        }
        let launch = legacy.launch_spec()?;
        let mut record = AgentRecord {
            name: legacy.name,
            token: legacy.token,
            launch,
            created_at: legacy.created_at,
            lifecycle: legacy.lifecycle,
            workspace_id: legacy.workspace_id,
            tab_id: legacy.tab_id,
            pane_id: legacy.pane_id,
            session_agent: legacy.session_agent,
            session_value: legacy.session_value,
            session_source: legacy.session_source,
            startup_warning: legacy.startup_warning,
            effective_reasoning_effort: legacy.effective_reasoning_effort,
            error: legacy.error,
            goal: legacy.goal,
            goal_command: legacy.goal_command,
            goal_message_id: legacy.goal_message_id,
            paused: legacy.paused,
            pane_reported_by_agentctl: legacy.pane_reported_by_agentctl,
            custom_process_identity: legacy.custom_process_identity,
            foreign_shell_identity: legacy.foreign_shell_identity,
            runner_identity: legacy.runner_identity,
            legacy_runner_pid: None,
            legacy_runner_started_at: None,
            legacy_goal_delivery: legacy.goal_delivery,
            legacy_goal_messages: legacy.goal_messages,
            legacy_goal_pointer,
            extra: legacy.extra,
        };
        if let Some(identity) = record.runner_identity.as_ref() {
            if legacy.runner_pid.is_some_and(|pid| pid != identity.pid)
                || legacy
                    .runner_started_at
                    .as_ref()
                    .is_some_and(|started| started != &identity.starttime_ticks.to_string())
            {
                return Err(fail(format!(
                    "contradictory runner identity in {}",
                    path.display()
                )));
            }
        } else {
            record.legacy_runner_pid = legacy.runner_pid;
            record.legacy_runner_started_at = legacy.runner_started_at;
        }
        record.validate_loaded(path, agent_name)?;
        record.validate_runtime_shape(&record.launch)?;
        Ok(record)
    }

    fn validate_runtime_shape(&self, launch: &LaunchSpec) -> Result<()> {
        let valid_lifecycle = matches!(
            self.lifecycle.as_str(),
            "starting" | "running" | "stopping" | "stopped" | "launch_failed" | "adopt_failed"
        );
        let expected_program = launch
            .executable
            .as_ref()
            .map_or(launch.harness.as_str(), |identity| identity.path.as_str());
        let option_valid = |value: Option<&str>| {
            value.is_none_or(|value| !value.is_empty() && !value.contains('\0'))
        };
        let mut launch_shape = Path::new(&launch.cwd).is_absolute()
            && option_valid(launch.model.as_deref())
            && option_valid(launch.resume.as_deref())
            && option_valid(launch.profile.as_deref())
            && launch
                .argv
                .iter()
                .all(|value| !value.is_empty() && !value.contains('\0'))
            && launch.environment_names.iter().all(|value| {
                value
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            && if launch.adapter == "herdr-foreign" {
                launch.argv.is_empty()
            } else {
                launch
                    .argv
                    .first()
                    .is_some_and(|value| value == expected_program)
            };
        // Interactive adapters launch the harness directly, so their
        // structured model/resume arguments must be present in argv.  The
        // Python turn-runner protocol carries those fields separately and
        // reserves argv for its harness-specific extra arguments.
        if matches!(launch.adapter.as_str(), "herdr" | "herdr-pane") {
            let structured = harness_arguments(
                &launch.harness,
                launch.model.as_deref(),
                launch.resume.as_deref(),
                &[],
            )?;
            launch_shape = launch_shape
                && launch.argv.get(1..1 + structured.len()) == Some(structured.as_slice());
        }
        let valid = if launch.adapter == "turn-runner" {
            launch.mode == "headless"
                && matches!(launch.backend.as_str(), "herdr" | "tmux")
                && launch.runtime_ownership == "owned"
                && launch
                    .runtime_home
                    .as_deref()
                    .is_some_and(|home| Path::new(home).is_absolute())
                && self.custom_process_identity.is_none()
                && self.foreign_shell_identity.is_none()
                && matches!(
                    launch.permission_mode.as_deref(),
                    None | Some("native") | Some("bypass")
                )
                && (launch.permission_mode.as_deref() != Some("bypass")
                    || launch.harness == "codex")
        } else {
            let base = matches!(
                launch.adapter.as_str(),
                "herdr" | "herdr-pane" | "herdr-foreign"
            ) && launch.mode == "interactive"
                && launch.backend == "herdr"
                && launch.runtime_home.is_none()
                && self.runner_identity.is_none()
                && self.legacy_runner_pid.is_none()
                && self.legacy_runner_started_at.is_none()
                && launch.permission_mode.is_none();
            base && match launch.adapter.as_str() {
                "herdr" => {
                    launch.runtime_ownership == "owned"
                        && self.custom_process_identity.is_none()
                        && self.foreign_shell_identity.is_none()
                }
                "herdr-pane" => {
                    launch.runtime_ownership == "owned"
                        && launch.harness == "muse"
                        && self.foreign_shell_identity.is_none()
                }
                "herdr-foreign" => {
                    launch.runtime_ownership == "foreign" && self.custom_process_identity.is_none()
                }
                _ => false,
            }
        };
        if !valid_lifecycle || !launch_shape || !valid {
            return Err(fail(
                "adapter, mode, backend, ownership, and runtime identity are inconsistent",
            ));
        }
        Ok(())
    }

    fn validate_loaded(&self, path: &Path, agent_name: &str) -> Result<()> {
        if self
            .session_agent
            .as_ref()
            .is_some_and(|agent| agent != &self.launch.harness)
        {
            return Err(fail(format!(
                "native session harness mismatch in {}",
                path.display()
            )));
        }
        if self.name != agent_name
            || self.token.is_empty()
            || self.token.len() > 80
            || !self
                .token
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || self.launch.harness.is_empty()
            || self.launch.cwd.is_empty()
            || self.lifecycle.is_empty()
            || !self.created_at.is_finite()
            || self
                .goal_message_id
                .as_deref()
                .is_some_and(|value| !message_id(value))
            || (self.goal_message_id.is_some() && self.goal.is_none())
            || !matches!(
                self.legacy_goal_delivery.as_deref(),
                None | Some("pending") | Some("possibly_submitted") | Some("delivered")
            )
            || self
                .legacy_goal_messages
                .iter()
                .any(|(key, value)| !message_id(key) || value.is_empty())
            || self.goal_message_id.as_ref().is_some_and(|identifier| {
                self.legacy_goal_messages
                    .get(identifier)
                    .is_some_and(|objective| Some(objective) != self.goal.as_ref())
            })
            || !matches!(
                self.session_source.as_deref(),
                None | Some("observed") | Some("asserted")
            )
            || (self.session_value.is_some() != self.session_source.is_some())
            || (self.session_value.is_some() != self.session_agent.is_some())
            || self.goal_command.as_ref().is_some_and(|command| {
                command.is_empty()
                    || command
                        .iter()
                        .any(|value| value.is_empty() || value.contains('\0'))
            })
            || self.startup_warning.as_deref().is_some_and(|warning| {
                warning.len() > 256 || !warning.is_ascii() || warning.contains(['\r', '\n', '\0'])
            })
            || self
                .effective_reasoning_effort
                .as_deref()
                .is_some_and(|effort| {
                    !matches!(
                        effort,
                        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
                    )
                })
            || self
                .custom_process_identity
                .as_ref()
                .is_some_and(|identity| {
                    self.launch.adapter != "herdr-pane"
                        || self.launch.harness != "muse"
                        || self.pane_id.as_deref().is_none_or(str::is_empty)
                        || !identity.valid()
                })
            || self
                .foreign_shell_identity
                .as_ref()
                .is_some_and(|identity| {
                    self.launch.adapter != "herdr-foreign"
                        || self.pane_id.as_deref().is_none_or(str::is_empty)
                        || !identity.valid()
                })
            || !matches!(self.launch.runtime_ownership.as_str(), "owned" | "foreign")
            || self
                .legacy_runner_pid
                .is_some_and(|pid| pid == 0 || pid > i32::MAX as u64)
            || self
                .legacy_runner_started_at
                .as_deref()
                .is_some_and(|started_at| {
                    started_at.is_empty()
                        || !started_at.bytes().all(|byte| byte.is_ascii_digit())
                        || started_at.bytes().all(|byte| byte == b'0')
                })
            || self.legacy_runner_pid.is_some() != self.legacy_runner_started_at.is_some()
            || self
                .runner_identity
                .as_ref()
                .is_some_and(|identity| !identity.valid())
            || self
                .launch
                .argv
                .iter()
                .any(|value| value.is_empty() || value.contains('\0'))
            || self.launch.environment_names.iter().any(|value| {
                value.is_empty()
                    || !value
                        .bytes()
                        .next()
                        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            || self.launch.executable.as_ref().is_some_and(|executable| {
                !Path::new(&executable.path).is_absolute()
                    || self.launch.argv.first() != Some(&executable.path)
                    || executable.device == 0
                    || executable.inode == 0
            })
        {
            return Err(fail(format!("invalid agent record: {}", path.display())));
        }
        Ok(())
    }

    fn supported(&self) -> Result<()> {
        if !matches!(
            self.launch.adapter.as_str(),
            "herdr" | "herdr-pane" | "herdr-foreign"
        ) || self.launch.mode != "interactive"
            || self.launch.backend != "herdr"
        {
            return Err(fail(format!("agent {:?} uses adapter {:?}, mode {:?}, backend {:?}; use the agentctl with the worker extension implementation for this runtime", self.name, self.launch.adapter, self.launch.mode, self.launch.backend)));
        }
        Ok(())
    }
    fn input_allowed(&self) -> Result<()> {
        self.supported()?;
        if self.paused {
            return Err(fail(format!("agent {:?} is paused for human input; run agentctl resume before sending automation input", self.name)));
        }
        Ok(())
    }

    fn target(&self) -> Result<Target> {
        self.supported()?;
        if self.pane_id.as_deref().is_none_or(str::is_empty) {
            return Err(fail(format!(
                "agent {:?} has no confirmed pane; inspect its launch error",
                self.name
            )));
        }
        let observed_session = self.session_source.as_deref() == Some("observed");
        Ok(Target {
            pane_id: self.pane_id.clone(),
            session_agent: observed_session
                .then(|| self.session_agent.clone())
                .flatten(),
            session_value: observed_session
                .then(|| self.session_value.clone())
                .flatten(),
            expected_agent: Some(self.launch.harness.clone()),
            expected_cwd: Some(PathBuf::from(&self.launch.cwd)),
            expected_workspace: None,
        })
    }
}

fn goal_replacement_selected(screen: &str, objective: &str) -> bool {
    let screen = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    let objective = objective.split_whitespace().collect::<Vec<_>>().join(" ");
    screen.contains("Replace goal?") && ["›", "❯"].iter().any(|marker| screen.contains(&format!("New objective: {objective} {marker} 1. Replace current goal Set the new objective and start it now")))
        && screen.contains("2. Cancel Keep the current goal") && screen.contains("Press enter to confirm or esc to go back")
}

fn goal_prompt(harness: &str, objective: &str) -> String {
    if harness == "codex" {
        format!("/goal {objective}")
    } else {
        format!(
            "Your ongoing goal: {objective}\nWork toward this goal and report completion or blockers."
        )
    }
}

#[derive(Clone, Debug)]
struct CustomPaneSubmission {
    harness: String,
    text: String,
    prior_transcript_count: usize,
    prior_active: bool,
}

struct WorkspaceClient<'a, A: ManagedApi + ?Sized> {
    client: &'a A,
    record: &'a AgentRecord,
    goal_objective: Mutex<Option<String>>,
    custom_submission: Mutex<Option<CustomPaneSubmission>>,
    queue: Option<&'a Path>,
    check_prompt: bool,
    adopted_evidence: Mutex<Option<AdoptedRuntimeEvidence>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdoptedRuntimeState {
    LiveExact,
    IdleShellExact,
    Missing,
    Ambiguous,
    IdentityMismatch,
    Unknown,
}

impl AdoptedRuntimeState {
    fn as_str(self) -> &'static str {
        match self {
            Self::LiveExact => "live-exact",
            Self::IdleShellExact => "idle-shell-exact",
            Self::Missing => "missing",
            Self::Ambiguous => "ambiguous",
            Self::IdentityMismatch => "identity-mismatch",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AdoptedRuntimeEvidence {
    state: AdoptedRuntimeState,
    reason_code: String,
    reason: String,
    info: Option<AgentPaneInfo>,
    presentation: Option<Pane>,
}

impl AdoptedRuntimeEvidence {
    fn public_value(&self) -> Value {
        json!({
            "state": self.state.as_str(),
            "reason_code": self.reason_code,
            "reason": self.reason,
        })
    }

    fn require_live(&self) -> crate::error::Result<AgentPaneInfo> {
        if self.state == AdoptedRuntimeState::LiveExact {
            if let Some(info) = &self.info {
                return Ok(info.clone());
            }
        }
        Err(crate::error::AdapterError::unavailable(&self.reason))
    }
}

struct OperationDeadlineRuntime<'a> {
    inner: &'a dyn agent::AgentRuntime,
    deadline: Duration,
}

impl agent::AgentRuntime for OperationDeadlineRuntime<'_> {
    fn monotonic(&self) -> Duration {
        self.inner.monotonic()
    }

    fn sleep(&self, duration: Duration) {
        self.inner
            .sleep(duration.min(self.deadline.saturating_sub(self.inner.monotonic())));
    }

    fn cancelled(&self) -> bool {
        self.inner.cancelled() || self.inner.monotonic() >= self.deadline
    }

    fn delivery_wait_chunk(&self) -> Option<Duration> {
        Some(
            self.inner
                .delivery_wait_chunk()
                .unwrap_or(Duration::from_secs(1))
                .min(self.deadline.saturating_sub(self.inner.monotonic())),
        )
    }
}

impl<A: ManagedApi + ?Sized> WorkspaceClient<'_, A> {
    fn adopted_evidence_with_runtime(
        &self,
        runtime: &dyn agent::AgentRuntime,
        refresh: bool,
    ) -> AdoptedRuntimeEvidence {
        if !refresh {
            if let Some(cached) = self
                .adopted_evidence
                .lock()
                .expect("adopted evidence lock poisoned")
                .clone()
            {
                return cached;
            }
        }
        let observed =
            |state, code: &str, reason: String, info, presentation| AdoptedRuntimeEvidence {
                state,
                reason_code: code.to_owned(),
                reason,
                info,
                presentation,
            };
        let evidence = if self.record.launch.adapter != "herdr-foreign" {
            observed(
                AdoptedRuntimeState::IdentityMismatch,
                "adopted-adapter-required",
                "adopted-runtime evidence requires herdr-foreign".to_owned(),
                None,
                None,
            )
        } else if self.record.pane_id.is_none() || self.record.foreign_shell_identity.is_none() {
            observed(
                AdoptedRuntimeState::Unknown,
                "runtime-probe-failed",
                format!(
                    "legacy record has no identity-bound pane shell for adopted agent {:?}",
                    self.record.name
                ),
                None,
                None,
            )
        } else {
            let pane_id = self.record.pane_id.as_deref().expect("checked pane");
            let shell = self
                .record
                .foreign_shell_identity
                .as_ref()
                .expect("checked shell identity");
            match self.client.panes_with_runtime(runtime) {
                Err(error) => observed(
                    AdoptedRuntimeState::Unknown,
                    "runtime-probe-failed",
                    error.to_string(),
                    None,
                    None,
                ),
                Ok(panes) => {
                    let matching = panes
                        .into_iter()
                        .filter(|pane| pane.pane_id == pane_id)
                        .collect::<Vec<_>>();
                    if matching.is_empty() {
                        observed(
                            AdoptedRuntimeState::Missing,
                            "pane-missing",
                            "expected one recorded pane, found 0".to_owned(),
                            None,
                            None,
                        )
                    } else if matching.len() != 1 {
                        observed(
                            AdoptedRuntimeState::Ambiguous,
                            "pane-identity-ambiguous",
                            format!("expected one recorded pane, found {}", matching.len()),
                            None,
                            None,
                        )
                    } else {
                        let presentation = matching[0].clone();
                        if Some(&presentation.workspace_id) != self.record.workspace_id.as_ref() {
                            observed(
                                AdoptedRuntimeState::IdentityMismatch,
                                "runtime-identity-mismatch",
                                format!("adopted pane {pane_id:?} workspace identity changed"),
                                None,
                                Some(presentation),
                            )
                        } else {
                            match self.client.pane_info_with_runtime(pane_id, runtime) {
                                Err(error) => observed(
                                    AdoptedRuntimeState::Unknown,
                                    "runtime-probe-failed",
                                    error.to_string(),
                                    None,
                                    Some(presentation),
                                ),
                                Ok(info) => {
                                    let cwd_matches = info.cwd == self.record.launch.cwd
                                        || fs::canonicalize(&info.cwd).ok().is_some_and(|cwd| {
                                            Some(cwd)
                                                == fs::canonicalize(&self.record.launch.cwd).ok()
                                        });
                                    if info.pane_id != pane_id
                                        || Some(&info.workspace_id)
                                            != self.record.workspace_id.as_ref()
                                        || !cwd_matches
                                    {
                                        observed(
                                            AdoptedRuntimeState::IdentityMismatch,
                                            "runtime-identity-mismatch",
                                            format!(
                                                "recorded pane, workspace, or cwd changed for {pane_id:?}"
                                            ),
                                            Some(info),
                                            Some(presentation),
                                        )
                                    } else if let Err(error) =
                                        self.client.verify_pane_shell_identity_with_runtime(
                                            pane_id, shell, runtime,
                                        )
                                    {
                                        let detail = error.to_string();
                                        let state = if detail.contains("changed") {
                                            AdoptedRuntimeState::IdentityMismatch
                                        } else {
                                            AdoptedRuntimeState::Unknown
                                        };
                                        observed(
                                            state,
                                            if state == AdoptedRuntimeState::IdentityMismatch {
                                                "runtime-identity-mismatch"
                                            } else {
                                                "runtime-probe-failed"
                                            },
                                            detail,
                                            Some(info),
                                            Some(presentation),
                                        )
                                    } else if info.agent.as_deref()
                                        == Some(self.record.launch.harness.as_str())
                                    {
                                        if self.record.session_source.as_deref() == Some("observed")
                                            && (self.record.session_agent.as_ref().is_some_and(
                                                |expected| {
                                                    info.session_agent.as_ref() != Some(expected)
                                                },
                                            ) || self
                                                .record
                                                .session_value
                                                .as_ref()
                                                .is_some_and(|expected| {
                                                    info.session_value.as_ref() != Some(expected)
                                                }))
                                        {
                                            observed(
                                                AdoptedRuntimeState::IdentityMismatch,
                                                "runtime-identity-mismatch",
                                                format!(
                                                    "adopted pane {pane_id:?} is not exactly one live pane with the recorded native session identity"
                                                ),
                                                Some(info),
                                                Some(presentation),
                                            )
                                        } else {
                                            observed(
                                                AdoptedRuntimeState::LiveExact,
                                                "ok",
                                                "adopted harness and shell generation are live and exact"
                                                    .to_owned(),
                                                Some(info),
                                                Some(presentation),
                                            )
                                        }
                                    } else if info.agent.is_some() {
                                        observed(
                                            AdoptedRuntimeState::IdentityMismatch,
                                            "expected-harness-missing",
                                            format!(
                                                "pane {pane_id:?} reports agent {:?}, expected {:?}",
                                                info.agent, self.record.launch.harness
                                            ),
                                            Some(info),
                                            Some(presentation),
                                        )
                                    } else if info.session_agent.is_some()
                                        || info.session_value.is_some()
                                    {
                                        observed(
                                            AdoptedRuntimeState::IdentityMismatch,
                                            "runtime-identity-mismatch",
                                            format!(
                                                "absent agent has native session identity in pane {pane_id:?}"
                                            ),
                                            Some(info),
                                            Some(presentation),
                                        )
                                    } else {
                                        match self.client.pane_is_same_idle_shell_with_runtime(
                                            pane_id, shell, runtime,
                                        ) {
                                            Ok(true)
                                                if Some(&presentation.tab_id)
                                                    != self.record.tab_id.as_ref() =>
                                            {
                                                observed(
                                                    AdoptedRuntimeState::IdentityMismatch,
                                                    "runtime-identity-mismatch",
                                                    format!(
                                                        "recorded tab changed while the agent was absent ({:?})",
                                                        self.record.name
                                                    ),
                                                    Some(info),
                                                    Some(presentation),
                                                )
                                            }
                                            Ok(true) => observed(
                                                AdoptedRuntimeState::IdleShellExact,
                                                "expected-harness-missing",
                                                format!(
                                                    "adopted pane {pane_id:?} returned to its exact recorded idle shell"
                                                ),
                                                Some(info),
                                                Some(presentation),
                                            ),
                                            Ok(false) => observed(
                                                AdoptedRuntimeState::Unknown,
                                                "agent-report-missing",
                                                format!(
                                                    "pane {pane_id:?} is not at the recorded identity-bound idle shell process group"
                                                ),
                                                Some(info),
                                                Some(presentation),
                                            ),
                                            Err(error) => observed(
                                                AdoptedRuntimeState::Unknown,
                                                "runtime-probe-failed",
                                                error.to_string(),
                                                Some(info),
                                                Some(presentation),
                                            ),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        };
        *self
            .adopted_evidence
            .lock()
            .expect("adopted evidence lock poisoned") = Some(evidence.clone());
        evidence
    }
}

impl<A: ManagedApi + ?Sized> AgentApi for WorkspaceClient<'_, A> {
    fn panes(&self) -> crate::error::Result<Vec<Pane>> {
        self.panes_with_runtime(&agent::SystemRuntime::default())
    }
    fn pane_info(&self, pane_id: &str) -> crate::error::Result<AgentPaneInfo> {
        self.pane_info_with_runtime(pane_id, &agent::SystemRuntime::default())
    }
    fn workspace_label(&self, workspace_id: &str) -> crate::error::Result<String> {
        self.workspace_label_with_runtime(workspace_id, &agent::SystemRuntime::default())
    }
    fn run(&self, pane_id: &str, text: &str) -> crate::error::Result<()> {
        self.run_with_runtime(pane_id, text, &agent::SystemRuntime::default())
    }
    fn wait_agent_status(
        &self,
        pane_id: &str,
        status: &str,
        timeout_ms: u64,
    ) -> crate::error::Result<()> {
        self.wait_agent_status_with_runtime(
            pane_id,
            status,
            timeout_ms,
            &agent::SystemRuntime::default(),
        )
    }
    fn read(
        &self,
        pane_id: &str,
        source: &str,
        lines: Option<usize>,
    ) -> crate::error::Result<String> {
        self.read_with_runtime(pane_id, source, lines, &agent::SystemRuntime::default())
    }

    fn panes_with_runtime(
        &self,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<Vec<Pane>> {
        if self.record.launch.adapter == "herdr-foreign" {
            let evidence = self.adopted_evidence_with_runtime(runtime, false);
            return Ok(evidence.presentation.into_iter().collect());
        }
        Ok(self
            .client
            .panes_with_runtime(runtime)?
            .into_iter()
            .filter(|pane| Some(&pane.workspace_id) == self.record.workspace_id.as_ref())
            .collect())
    }

    fn pane_info_with_runtime(
        &self,
        pane_id: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<AgentPaneInfo> {
        if self.record.launch.adapter == "herdr-foreign"
            && Some(pane_id) == self.record.pane_id.as_deref()
        {
            return self
                .adopted_evidence_with_runtime(runtime, false)
                .require_live();
        }
        if self.record.launch.adapter == "herdr"
            && Some(
                self.client
                    .agent_pane_with_runtime(&self.record.name, runtime)?,
            ) != self.record.pane_id
        {
            return Err(crate::error::AdapterError::unavailable(format!(
                "agent {:?} no longer owns its recorded pane",
                self.record.name
            )));
        }
        let mut info = self.client.pane_info_with_runtime(pane_id, runtime)?;
        if Some(&info.workspace_id) != self.record.workspace_id.as_ref() {
            return Err(crate::error::AdapterError::unavailable(format!(
                "agent {:?} workspace identity changed",
                self.record.name
            )));
        }
        if Some(pane_id) == self.record.pane_id.as_deref() {
            if self.record.session_source.as_deref() == Some("observed")
                && self.record.session_value.is_some()
                && info.session_value.is_some()
                && info.session_value != self.record.session_value
            {
                return Err(crate::error::AdapterError::unavailable(format!(
                    "agent {:?} native session identity changed",
                    self.record.name
                )));
            }
            if matches!(info.status.as_str(), "idle" | "done")
                && info.agent.as_deref() == Some("claude")
            {
                match self
                    .client
                    .read_with_runtime(pane_id, "visible", Some(200), runtime)
                {
                    Ok(screen) => {
                        if self.check_prompt
                            && screen.contains("Quick safety check: Is this a project you created or one you trust?")
                            && screen.contains("No, exit")
                            && screen.contains("Yes, I trust this folder")
                        {
                            return Err(crate::error::AdapterError::unavailable("Claude workspace trust prompt requires human attention; no input was submitted"));
                        }
                        if claude_staged_composer(&screen) {
                            info.status = "staged".to_owned();
                        } else if claude_active_screen(&screen) {
                            info.status = "working".to_owned();
                        }
                    }
                    Err(error) if self.check_prompt => return Err(error),
                    Err(_) => {}
                }
            }
            if self.record.launch.adapter == "herdr-pane" {
                let reported_status = info.status.clone();
                self.client.verify_custom_harness_with_runtime(
                    pane_id,
                    &self.record.launch.harness,
                    self.record.custom_process_identity.as_ref(),
                    runtime,
                )?;
                let screen =
                    self.client
                        .read_with_runtime(pane_id, "visible", Some(200), runtime)?;
                if muse_trust_prompt(&screen) {
                    return Err(crate::error::AdapterError::unavailable(
                        "Muse workspace trust prompt requires human attention; no input was submitted",
                    ));
                }
                // The exact custom-process proof is authoritative even when
                // Herdr's advisory agent label has not been published yet.
                info.agent = Some(self.record.launch.harness.clone());
                info.status = if muse_idle_composer(&screen)
                    || (matches!(reported_status.as_str(), "idle" | "done")
                        && muse_verified_process_idle_composer(&screen))
                {
                    "idle".to_owned()
                } else if matches!(reported_status.as_str(), "idle" | "done")
                    && muse_verified_process_composer(&screen)
                {
                    "staged".to_owned()
                } else {
                    "working".to_owned()
                };
            }
        }
        Ok(info)
    }

    fn workspace_label_with_runtime(
        &self,
        workspace_id: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<String> {
        self.client
            .workspace_label_with_runtime(workspace_id, runtime)
    }

    fn run_with_runtime(
        &self,
        pane_id: &str,
        text: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        if runtime.cancelled() {
            return Err(crate::error::AdapterError::unavailable(
                "Herdr control operation was cancelled",
            ));
        }
        *self
            .goal_objective
            .lock()
            .expect("goal operation lock poisoned") = None;
        if let (Some(queue), Some(objective)) = (self.queue, text.strip_prefix("/goal ")) {
            let inflight = queue.join("inflight");
            let mut entries = fs::read_dir(&inflight)
                .map_err(|error| crate::error::AdapterError::unavailable(error.to_string()))?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|error| crate::error::AdapterError::unavailable(error.to_string()))?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let document = agent::read_private_json(&entry.path())
                    .map_err(|error| crate::error::AdapterError::unavailable(error.to_string()))?;
                if document["kind"].as_str() == Some("goal")
                    && document["text"].as_str() == Some(text)
                {
                    *self
                        .goal_objective
                        .lock()
                        .expect("goal operation lock poisoned") = Some(objective.to_owned());
                    break;
                }
            }
        }
        if self.record.launch.adapter != "herdr-pane" {
            if self.record.launch.harness == "claude" {
                let before =
                    self.client
                        .read_with_runtime(pane_id, "visible", Some(200), runtime)?;
                let prior_transcript_count = claude_prompt_transcript_count(&before, text);
                let prior_active = claude_active_screen(&before);
                if !claude_prompt_is_exact_composer(&before, text) {
                    if claude_staged_composer(&before) {
                        return Err(crate::error::AdapterError::unavailable(
                            "Claude editor contains different buffered input; no input was sent",
                        ));
                    }
                    self.client.run_with_runtime(pane_id, text, runtime)?;
                }
                let staged =
                    self.client
                        .read_with_runtime(pane_id, "visible", Some(200), runtime)?;
                if !claude_prompt_is_exact_composer(&staged, text) {
                    // Preserve Herdr's native transition contract when the
                    // screen does not prove a staged Claude editor. It never
                    // authorizes a second input operation.
                    return Ok(());
                }
                if self.pane_info_with_runtime(pane_id, runtime)?.status != "staged" {
                    return Err(crate::error::AdapterError::unavailable(
                        "Claude staged prompt changed before its submit chord",
                    ));
                }
                self.client
                    .send_keys_with_runtime(pane_id, "ctrl+x ctrl+s", runtime)?;
                *self
                    .custom_submission
                    .lock()
                    .expect("custom submission lock poisoned") = Some(CustomPaneSubmission {
                    harness: "claude".to_owned(),
                    text: text.to_owned(),
                    prior_transcript_count,
                    prior_active,
                });
                return Ok(());
            }
            return self.client.run_with_runtime(pane_id, text, runtime);
        }
        if text.contains(['\0', '\u{1b}']) {
            return Err(crate::error::AdapterError::unavailable(
                "Muse pane prompts cannot contain NUL or terminal escape characters",
            ));
        }
        let info = self.pane_info_with_runtime(pane_id, runtime)?;
        if !matches!(info.status.as_str(), "idle" | "staged") {
            return Err(crate::error::AdapterError::unavailable(format!(
                "custom pane {pane_id} is not at a verified idle or staged Muse composer"
            )));
        }
        let before =
            self.client
                .read_with_runtime(pane_id, "recent-unwrapped", Some(200), runtime)?;
        if !muse_verified_process_composer(&before) {
            return Err(crate::error::AdapterError::unavailable(
                "current Muse editor could not be verified before delivery; no input was sent",
            ));
        }
        if muse_verified_process_prompt_is_exact_composer(&before, text) {
            // The exact requested input is already staged. Re-prove it below
            // rather than injecting a duplicate paste.
        } else {
            if !muse_verified_process_idle_composer(&before) {
                return Err(crate::error::AdapterError::unavailable(
                    "Muse editor contains different buffered input; no input or Enter was sent",
                ));
            }
            self.client.send_text_with_runtime(
                pane_id,
                &format!("{BRACKETED_PASTE_START}{text}{BRACKETED_PASTE_END}"),
                runtime,
            )?;
            let deadline = Instant::now() + Duration::from_secs(2);
            let _staged = loop {
                if runtime.cancelled() {
                    return Err(crate::error::AdapterError::unavailable(
                        "Herdr control operation was cancelled",
                    ));
                }
                self.client.verify_custom_harness_with_runtime(
                    pane_id,
                    &self.record.launch.harness,
                    self.record.custom_process_identity.as_ref(),
                    runtime,
                )?;
                let screen = self.client.read_with_runtime(
                    pane_id,
                    "recent-unwrapped",
                    Some(200),
                    runtime,
                )?;
                if screen != before && muse_verified_process_prompt_is_exact_composer(&screen, text)
                {
                    break screen;
                }
                if Instant::now() >= deadline {
                    return Err(crate::error::AdapterError::unavailable(
                        "literal text insertion did not produce exact visible Muse editor evidence; Enter was not sent",
                    ));
                }
                std::thread::sleep(
                    Duration::from_millis(50)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            };
        }
        self.client.verify_custom_harness_with_runtime(
            pane_id,
            &self.record.launch.harness,
            self.record.custom_process_identity.as_ref(),
            runtime,
        )?;
        let confirmed =
            self.client
                .read_with_runtime(pane_id, "recent-unwrapped", Some(200), runtime)?;
        if !muse_verified_process_prompt_is_exact_composer(&confirmed, text) {
            return Err(crate::error::AdapterError::unavailable(
                "Muse editor changed before submission; Enter was not sent",
            ));
        }
        self.client.verify_custom_harness_with_runtime(
            pane_id,
            &self.record.launch.harness,
            self.record.custom_process_identity.as_ref(),
            runtime,
        )?;
        self.client
            .send_keys_with_runtime(pane_id, "Enter", runtime)?;
        *self
            .custom_submission
            .lock()
            .expect("custom submission lock poisoned") = Some(CustomPaneSubmission {
            harness: "muse".to_owned(),
            prior_transcript_count: muse_verified_process_prompt_transcript_count(&confirmed, text),
            text: text.to_owned(),
            prior_active: false,
        });
        Ok(())
    }

    fn wait_agent_status_with_runtime(
        &self,
        pane_id: &str,
        status: &str,
        timeout_ms: u64,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<()> {
        let deadline = runtime
            .monotonic()
            .saturating_add(Duration::from_millis(timeout_ms));
        let bounded = OperationDeadlineRuntime {
            inner: runtime,
            deadline,
        };
        let submission = self
            .custom_submission
            .lock()
            .expect("custom submission lock poisoned")
            .clone();
        if let ("working", Some(submission)) = (status, submission) {
            let retry_after = runtime.monotonic().saturating_add(
                Duration::from_millis(250).min(Duration::from_millis(timeout_ms / 2)),
            );
            let mut retried_enter = false;
            loop {
                if runtime.cancelled() {
                    return Err(crate::error::AdapterError::unavailable(
                        "Herdr control operation was cancelled",
                    ));
                }
                if bounded.monotonic() >= deadline {
                    return Err(crate::error::AdapterError::unavailable(
                        "agent did not show a verified post-submission screen transition",
                    ));
                }
                let (screen, transcript_count, in_composer) = if submission.harness == "muse" {
                    self.client.verify_custom_harness_with_runtime(
                        pane_id,
                        &self.record.launch.harness,
                        self.record.custom_process_identity.as_ref(),
                        &bounded,
                    )?;
                    let screen = self.client.read_with_runtime(
                        pane_id,
                        "recent-unwrapped",
                        Some(200),
                        &bounded,
                    )?;
                    let count =
                        muse_verified_process_prompt_transcript_count(&screen, &submission.text);
                    let retained =
                        muse_verified_process_prompt_in_composer(&screen, &submission.text);
                    (screen, count, retained)
                } else {
                    self.pane_info_with_runtime(pane_id, &bounded)?;
                    let screen = self.client.read_with_runtime(
                        pane_id,
                        "recent-unwrapped",
                        Some(5000),
                        &bounded,
                    )?;
                    let count = claude_prompt_transcript_count(&screen, &submission.text);
                    let retained = claude_prompt_is_exact_composer(&screen, &submission.text);
                    (screen, count, retained)
                };
                if transcript_count > submission.prior_transcript_count && !in_composer {
                    *self
                        .custom_submission
                        .lock()
                        .expect("custom submission lock poisoned") = None;
                    return Ok(());
                }
                if submission.harness == "claude"
                    && !submission.prior_active
                    && !in_composer
                    && claude_active_screen(&screen)
                {
                    *self
                        .custom_submission
                        .lock()
                        .expect("custom submission lock poisoned") = None;
                    return Ok(());
                }
                if submission.harness == "muse"
                    && !retried_enter
                    && bounded.monotonic() >= retry_after
                    && transcript_count == submission.prior_transcript_count
                    && muse_verified_process_prompt_is_exact_composer(&screen, &submission.text)
                {
                    self.client.verify_custom_harness_with_runtime(
                        pane_id,
                        &self.record.launch.harness,
                        self.record.custom_process_identity.as_ref(),
                        &bounded,
                    )?;
                    self.client
                        .send_keys_with_runtime(pane_id, "Enter", &bounded)?;
                    retried_enter = true;
                }
                bounded.sleep(Duration::from_millis(50));
            }
        }
        let objective = self
            .goal_objective
            .lock()
            .expect("goal operation lock poisoned")
            .clone()
            .filter(|_| status == "working");
        let start = bounded.monotonic();
        let initial_error = match self.client.wait_agent_status_with_runtime(
            pane_id,
            status,
            timeout_ms.min(1000),
            &bounded,
        ) {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        // Herdr's wait subscription may be installed after the transition it
        // is waiting for. Reconcile the event failure against the exact owned
        // pane, without ever running the prompt operation a second time.
        if self.pane_info_with_runtime(pane_id, &bounded)?.status == status {
            return Ok(());
        }
        if let Some(objective) = objective {
            let screen = self
                .client
                .read_with_runtime(pane_id, "visible", Some(200), &bounded)?;
            if goal_replacement_selected(&screen, &objective) {
                self.client
                    .send_keys_with_runtime(pane_id, "Enter", &bounded)?;
            }
        }
        let elapsed = u64::try_from(bounded.monotonic().saturating_sub(start).as_millis())
            .unwrap_or(u64::MAX);
        let remaining = timeout_ms.saturating_sub(elapsed);
        if remaining == 0 || bounded.cancelled() {
            return Err(initial_error);
        }
        match self
            .client
            .wait_agent_status_with_runtime(pane_id, status, remaining, &bounded)
        {
            Ok(()) => Ok(()),
            Err(final_error) => {
                if self.pane_info_with_runtime(pane_id, &bounded)?.status == status {
                    Ok(())
                } else {
                    Err(crate::error::AdapterError::unavailable(format!(
                        "{final_error}; initial status wait also failed: {initial_error}"
                    )))
                }
            }
        }
    }

    fn read_with_runtime(
        &self,
        pane_id: &str,
        source: &str,
        lines: Option<usize>,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<String> {
        self.client
            .read_with_runtime(pane_id, source, lines, runtime)
    }
}

/// Options for a new visible subagent; the working directory is always explicit.
#[derive(Clone, Debug)]
pub struct StartOptions {
    /// Owner-selected launch profile name, without its secret values.
    pub launch_profile: Option<String>,
    /// Existing workspace ID, or the current workspace / shared `subagents` default.
    pub workspace_id: Option<String>,
    /// Unique workspace label, mutually exclusive with `workspace_id`.
    pub workspace_label: Option<String>,
    /// Herdr agent kind.
    pub harness: String,
    /// Optional model override for Codex or Claude.
    pub model: Option<String>,
    /// Optional saved conversation to resume.
    pub resume: Option<String>,
    /// Additional literal harness arguments.
    pub harness_args: Vec<String>,
    /// Literal environment entries for the new Herdr tab.
    pub environment: Vec<String>,
    /// Initial task to submit after the harness is ready.
    pub brief: Option<String>,
    /// Herdr's bounded startup-readiness deadline.
    pub startup_timeout: Duration,
    /// Delivery options for the initial task.
    pub delivery: DrainOptions,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            launch_profile: None,
            workspace_id: None,
            workspace_label: None,
            harness: "codex".to_owned(),
            model: None,
            resume: None,
            harness_args: Vec::new(),
            environment: Vec::new(),
            brief: None,
            startup_timeout: Duration::from_secs(30),
            delivery: DrainOptions::default(),
        }
    }
}

/// Required live-identity assertions for a pre-existing Herdr agent.
#[derive(Clone, Debug)]
pub struct AdoptOptions {
    /// Exact live pane containing the harness.
    pub pane_id: String,
    /// Expected live workspace label.
    pub expected_workspace: String,
    /// Expected live harness working directory.
    pub cwd: PathBuf,
    /// Expected Herdr harness kind.
    pub harness: String,
    /// Optional native session ID already reported by the exact pane.
    pub session: Option<String>,
}

/// Explicit safety assertions for stop/recovery.
#[derive(Clone, Debug, Default)]
pub struct StopOptions {
    /// Exact current registry generation token.
    pub expected_token: Option<String>,
    /// Enable the narrowly scoped identity-less adopted-row recovery path.
    pub recover_legacy_adoption: bool,
    /// SHA-256 of the exact current identity-less `agent.json` bytes.
    pub expected_record_sha256: Option<String>,
}

/// Registry-backed coordinator interface to visible foreign-harness workers.
pub struct ManagedAgents<'a, A: ManagedApi + ?Sized> {
    client: &'a A,
    registry: PathBuf,
    /// Workspace inherited from the launching Herdr pane (`HERDR_WORKSPACE_ID`),
    /// used when a start names none.
    inherited_workspace: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct DeadlineRuntime {
    origin: Instant,
    deadline: Instant,
}

impl DeadlineRuntime {
    fn until(deadline: Instant) -> Self {
        Self {
            origin: Instant::now(),
            deadline,
        }
    }
}

impl agent::AgentRuntime for DeadlineRuntime {
    fn monotonic(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration.min(self.deadline.saturating_duration_since(Instant::now())));
    }

    fn cancelled(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

impl<'a, A: ManagedApi + ?Sized> ManagedAgents<'a, A> {
    /// Use an explicit registry, independent of the agents' working directories.
    pub fn new(client: &'a A, registry: &Path) -> Result<Self> {
        let registry = if registry.is_absolute() {
            registry.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| fail(error.to_string()))?
                .join(registry)
        };
        let inherited_workspace = std::env::var("HERDR_WORKSPACE_ID")
            .ok()
            .filter(|s| !s.is_empty());
        Ok(Self {
            client,
            registry,
            inherited_workspace,
        })
    }

    /// Replace the inherited workspace, so tests never depend on the pane that runs them.
    #[cfg(test)]
    fn with_inherited_workspace(mut self, workspace: Option<&str>) -> Self {
        self.inherited_workspace = workspace.map(str::to_owned);
        self
    }

    fn directory(&self, agent_name: &str) -> Result<PathBuf> {
        Ok(self.registry.join(name(agent_name)?))
    }

    fn lock(&self, agent_name: &str) -> Result<File> {
        self.lock_with_runtime(agent_name, &agent::SystemRuntime::default())
    }

    fn try_lock_with_runtime(
        &self,
        agent_name: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<Option<File>> {
        name(agent_name)?;
        agent::create_private_directory(&self.registry, "agent registry", true, true)?;
        let path = self.registry.join(format!(".{agent_name}.lock"));
        let file = agent::open_private_lock(&path, "agent lifecycle lock")?;
        let lock_deadline = Instant::now() + Duration::from_millis(50);
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Some(file)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if runtime.cancelled() || Instant::now() >= lock_deadline {
                        return Ok(None);
                    }
                    runtime.sleep(
                        Duration::from_millis(5)
                            .min(lock_deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(error) => {
                    return Err(fail(format!(
                        "cannot lock agent lifecycle {}: {error}",
                        path.display()
                    )))
                }
            }
        }
    }

    fn lock_with_runtime(
        &self,
        agent_name: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<File> {
        name(agent_name)?;
        agent::create_private_directory(&self.registry, "agent registry", true, true)?;
        let path = self.registry.join(format!(".{agent_name}.lock"));
        let file = agent::open_private_lock(&path, "agent lifecycle lock")?;
        agent::lock_exclusive_with_runtime(&file, &path, "agent lifecycle", runtime)?;
        Ok(file)
    }

    fn identity_lock(&self) -> Result<File> {
        agent::create_private_directory(&self.registry, "agent registry", true, true)?;
        let file =
            agent::open_private_lock(&self.registry.join(".identity.lock"), "agent identity lock")?;
        file.lock_exclusive()
            .map_err(|error| fail(error.to_string()))?;
        Ok(file)
    }

    fn pane_lock(&self, pane_id: &str) -> Result<File> {
        let path = agent::target_lock_path(pane_id)?;
        let file = agent::open_private_lock(&path, "host-wide target lock")?;
        file.lock_exclusive()
            .map_err(|error| fail(error.to_string()))?;
        Ok(file)
    }

    fn private_directory_identity(metadata: &fs::Metadata, label: &str) -> Result<(u64, u64)> {
        let uid = unsafe { libc::getuid() };
        if !metadata.is_dir() || metadata.uid() != uid || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(fail(format!("{label} is not a private directory")));
        }
        Ok((metadata.dev(), metadata.ino()))
    }

    fn pinned_agent_directory(&self, agent_name: &str) -> Result<PinnedAgentDirectory> {
        self.pinned_agent_directory_with(agent_name, || {})
    }

    fn pinned_agent_directory_with(
        &self,
        agent_name: &str,
        after_preopen_check: impl FnOnce(),
    ) -> Result<PinnedAgentDirectory> {
        let path = self.directory(agent_name)?;
        Self::pinned_agent_directory_at_with(
            agent_name,
            path,
            "agent directory",
            after_preopen_check,
        )
    }

    fn pinned_agent_directory_at(
        agent_name: &str,
        path: PathBuf,
        label: &str,
    ) -> Result<PinnedAgentDirectory> {
        Self::pinned_agent_directory_at_with(agent_name, path, label, || {})
    }

    fn pinned_agent_directory_at_with(
        agent_name: &str,
        path: PathBuf,
        label: &str,
        after_preopen_check: impl FnOnce(),
    ) -> Result<PinnedAgentDirectory> {
        agent::validate_private_directory(&path, label, false)?;
        let before = fs::symlink_metadata(&path)
            .map_err(|error| fail(format!("cannot inspect {label}: {error}")))?;
        let (device, inode) = Self::private_directory_identity(&before, label)?;
        after_preopen_check();
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|error| fail(format!("cannot pin {label}: {error}")))?;
        let metadata = file
            .metadata()
            .map_err(|error| fail(format!("cannot inspect pinned {label}: {error}")))?;
        let opened = Self::private_directory_identity(&metadata, label)?;
        if opened != (device, inode) {
            return Err(fail(format!("{label} changed while being pinned")));
        }
        let pinned = PinnedAgentDirectory {
            name: agent_name.to_owned(),
            path,
            file,
            device,
            inode,
        };
        Self::verify_pinned_agent_directory(&pinned)?;
        Ok(pinned)
    }

    fn verify_pinned_agent_directory(pinned: &PinnedAgentDirectory) -> Result<()> {
        let path = fs::symlink_metadata(&pinned.path)
            .map_err(|error| fail(format!("agent registry directory changed: {error}")))?;
        let opened = pinned
            .file
            .metadata()
            .map_err(|error| fail(format!("cannot reinspect pinned agent directory: {error}")))?;
        let path_identity = Self::private_directory_identity(&path, "agent directory")?;
        let opened_identity = Self::private_directory_identity(&opened, "pinned agent directory")?;
        if path_identity != (pinned.device, pinned.inode)
            || opened_identity != (pinned.device, pinned.inode)
        {
            return Err(fail(format!(
                "agent {:?} registry directory changed",
                pinned.name
            )));
        }
        Ok(())
    }

    fn pinned_parent_directory(path: &Path, label: &str) -> Result<PinnedParentDirectory> {
        Self::pinned_parent_directory_with(path, label, || {})
    }

    fn pinned_parent_directory_with(
        path: &Path,
        label: &str,
        after_preopen_check: impl FnOnce(),
    ) -> Result<PinnedParentDirectory> {
        agent::validate_private_directory(path, label, false)?;
        let before = fs::symlink_metadata(path)
            .map_err(|error| fail(format!("cannot inspect {label}: {error}")))?;
        let (device, inode) = Self::private_directory_identity(&before, label)?;
        after_preopen_check();
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| fail(format!("cannot pin {label}: {error}")))?;
        let metadata = file
            .metadata()
            .map_err(|error| fail(format!("cannot inspect pinned {label}: {error}")))?;
        let opened = Self::private_directory_identity(&metadata, &format!("pinned {label}"))?;
        if opened != (device, inode) {
            return Err(fail(format!("{label} changed while being pinned")));
        }
        let pinned = PinnedParentDirectory {
            path: path.to_owned(),
            file,
            device,
            inode,
        };
        Self::verify_pinned_parent_directory(&pinned, label)?;
        Ok(pinned)
    }

    fn verify_pinned_parent_directory(pinned: &PinnedParentDirectory, label: &str) -> Result<()> {
        let path = fs::symlink_metadata(&pinned.path)
            .map_err(|error| fail(format!("{label} changed: {error}")))?;
        let opened = pinned
            .file
            .metadata()
            .map_err(|error| fail(format!("cannot reinspect pinned {label}: {error}")))?;
        let path_identity = Self::private_directory_identity(&path, label)?;
        let opened_identity =
            Self::private_directory_identity(&opened, &format!("pinned {label}"))?;
        if path_identity != (pinned.device, pinned.inode)
            || opened_identity != (pinned.device, pinned.inode)
        {
            return Err(fail(format!("{label} changed")));
        }
        Ok(())
    }

    fn child_directory_identity(parent: &File, name: &str) -> Result<Option<(u64, u64)>> {
        let name = CString::new(name).map_err(|_| fail("registry entry name contains NUL"))?;
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_RDONLY,
            )
        };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(fail(format!(
                "cannot inspect registry directory entry: {error}"
            )));
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        let metadata = file
            .metadata()
            .map_err(|error| fail(format!("cannot inspect registry directory entry: {error}")))?;
        Ok(Some(Self::private_directory_identity(
            &metadata,
            "registry directory entry",
        )?))
    }

    fn open_pinned_file(
        pinned: &PinnedAgentDirectory,
        name: &str,
        flags: libc::c_int,
    ) -> Result<File> {
        let name = CString::new(name).map_err(|_| fail("agent record name contains NUL"))?;
        let descriptor = unsafe {
            libc::openat(
                pinned.file.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if descriptor < 0 {
            return Err(fail(format!(
                "cannot open pinned agent file: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    fn open_optional_pinned_file(
        pinned: &PinnedAgentDirectory,
        name: &str,
        flags: libc::c_int,
    ) -> Result<Option<File>> {
        let name = CString::new(name).map_err(|_| fail("agent record name contains NUL"))?;
        let descriptor = unsafe {
            libc::openat(
                pinned.file.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if descriptor >= 0 {
            return Ok(Some(unsafe { File::from_raw_fd(descriptor) }));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        Err(fail(format!("cannot open pinned agent file: {error}")))
    }

    fn record_bytes(&self, pinned: &PinnedAgentDirectory) -> Result<Vec<u8>> {
        self.record_bytes_with(pinned, true)
    }

    fn record_bytes_with(
        &self,
        pinned: &PinnedAgentDirectory,
        require_active_name: bool,
    ) -> Result<Vec<u8>> {
        Self::pinned_artifact_bytes(
            pinned,
            "agent.json",
            MAX_AGENT_RECORD_BYTES,
            "agent record",
            require_active_name,
        )
    }

    fn pinned_artifact_bytes(
        pinned: &PinnedAgentDirectory,
        name: &str,
        limit: usize,
        label: &str,
        require_named_directory: bool,
    ) -> Result<Vec<u8>> {
        if require_named_directory {
            Self::verify_pinned_agent_directory(pinned)?;
        }
        let path = pinned.path.join(name);
        let mut file = Self::open_pinned_file(pinned, name, libc::O_RDONLY)?;
        let before = file.metadata().map_err(|error| {
            fail(format!(
                "cannot inspect {label} {}: {error}",
                path.display()
            ))
        })?;
        let uid = unsafe { libc::getuid() };
        if !before.is_file()
            || before.uid() != uid
            || before.permissions().mode() & 0o077 != 0
            || before.nlink() != 1
            || before.len() > limit as u64
        {
            return Err(fail(format!("unsafe {label}: {}", path.display())));
        }
        let mut content = Vec::with_capacity(before.len() as usize);
        Read::by_ref(&mut file)
            .take((limit + 1) as u64)
            .read_to_end(&mut content)
            .map_err(|error| fail(format!("cannot read {label} {}: {error}", path.display())))?;
        let after = file.metadata().map_err(|error| {
            fail(format!(
                "cannot reinspect {label} {}: {error}",
                path.display()
            ))
        })?;
        if content.len() > limit
            || before.len() != content.len() as u64
            || after.dev() != before.dev()
            || after.ino() != before.ino()
            || after.mode() != before.mode()
            || after.uid() != before.uid()
            || after.nlink() != before.nlink()
            || after.len() != before.len()
            || after.mtime() != before.mtime()
            || after.mtime_nsec() != before.mtime_nsec()
            || after.ctime() != before.ctime()
            || after.ctime_nsec() != before.ctime_nsec()
        {
            return Err(fail(format!(
                "{label} changed while reading: {}",
                path.display()
            )));
        }
        if require_named_directory {
            Self::verify_pinned_agent_directory(pinned)?;
        }
        Ok(content)
    }

    fn load(&self, agent_name: &str) -> Result<AgentRecord> {
        let directory = self.directory(agent_name)?;
        agent::validate_private_directory(&self.registry, "agent registry", false)?;
        if !directory.exists() {
            return Err(fail(format!(
                "unknown agent {agent_name:?}; use list to inspect the registry"
            )));
        }
        agent::validate_private_directory(&directory, "agent directory", false)?;
        let path = directory.join("agent.json");
        AgentRecord::from_storage_value(
            agent::read_private_json_bounded(&path, MAX_AGENT_RECORD_BYTES as u64)?,
            &path,
            agent_name,
        )
    }

    fn legacy_record_snapshot(
        &self,
        pinned: &PinnedAgentDirectory,
        expected_token: &str,
        expected_digest: &str,
    ) -> Result<LegacyRecordSnapshot> {
        if expected_digest.len() != 64
            || !expected_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(fail(
                "expected-record-sha256 must be exactly 64 lowercase hexadecimal characters",
            ));
        }
        let path = pinned.path.join("agent.json");
        let content = self.record_bytes(pinned)?;
        let document: Value = agent::decode_json_strict(&content).map_err(|error| {
            fail(format!(
                "cannot inspect legacy agent record {}: {error}",
                path.display()
            ))
        })?;
        let object = document
            .as_object()
            .ok_or_else(|| fail(format!("invalid legacy agent record: {}", path.display())))?;
        if object.get("schema").and_then(Value::as_u64) != Some(1)
            || object.contains_key("foreign_shell_identity")
        {
            return Err(fail(
                "--recover-legacy-adoption requires foreign_shell_identity to be absent, not null or populated, in a v1 record",
            ));
        }
        let record = AgentRecord::from_storage_value(document, &path, &pinned.name)?;
        let digest = format!("{:x}", Sha256::digest(&content));
        if record.token != expected_token || digest != expected_digest {
            return Err(fail(format!(
                "agent {:?} record changed before adoption recovery",
                pinned.name
            )));
        }
        Ok(LegacyRecordSnapshot {
            record,
            content,
            digest,
            directory_device: pinned.device,
            directory_inode: pinned.inode,
        })
    }

    fn managed_record_snapshot(
        &self,
        pinned: &PinnedAgentDirectory,
        expected_token: &str,
    ) -> Result<ManagedRecordSnapshot> {
        let path = pinned.path.join("agent.json");
        let content = self.record_bytes(pinned)?;
        let document: Value = agent::decode_json_strict(&content)
            .map_err(|error| fail(format!("invalid agent record {}: {error}", path.display())))?;
        let record = AgentRecord::from_storage_value(document, &path, &pinned.name)?;
        if record.token != expected_token {
            return Err(fail(format!(
                "agent {:?} was replaced before this operation",
                pinned.name
            )));
        }
        Ok(ManagedRecordSnapshot {
            record,
            content,
            directory_device: pinned.device,
            directory_inode: pinned.inode,
        })
    }

    fn save(&self, record: &AgentRecord) -> Result<()> {
        let document = record.storage_value()?;
        let encoded = serde_json::to_vec_pretty(&document)
            .map_err(|error| fail(format!("cannot encode agent record: {error}")))?;
        if encoded.len().saturating_add(1) > MAX_AGENT_RECORD_BYTES {
            return Err(fail(format!(
                "agent record exceeds {MAX_AGENT_RECORD_BYTES} bytes"
            )));
        }
        self.migrate_legacy_goal_messages(record)?;
        if record.goal_message_id.is_some() {
            self.goal_artifact_state(record, true)?;
        }
        agent::atomic_json(&self.directory(&record.name)?.join("agent.json"), &document)
    }

    fn migrate_legacy_goal_messages(&self, record: &AgentRecord) -> Result<()> {
        let mut legacy_goals = record.legacy_goal_messages.clone();
        if record.legacy_goal_pointer {
            if let Some(identifier) = &record.goal_message_id {
                let objective = record
                    .goal
                    .as_ref()
                    .ok_or_else(|| fail("legacy goal pointer has no session objective"))?;
                if legacy_goals
                    .get(identifier)
                    .is_some_and(|mapped| mapped != objective)
                {
                    return Err(fail(
                        "legacy goal pointer disagrees with its duplicate objective map",
                    ));
                }
                legacy_goals.insert(identifier.clone(), objective.clone());
            }
        }
        if legacy_goals.is_empty() {
            return Ok(());
        }
        let queue = self.queue(&record.name)?;
        if fs::symlink_metadata(&queue).is_err() {
            return Err(fail("legacy goal migration requires its durable queue"));
        }
        let lock_path = queue.join(".delivery.lock");
        let lock = agent::open_private_lock(&lock_path, "queue delivery lock")?;
        lock.try_lock_exclusive().map_err(|error| {
            fail(format!(
                "legacy goal migration is busy with queue delivery; retry: {error}"
            ))
        })?;
        for (identifier, objective) in &legacy_goals {
            let Some(state) = agent::message_state(&queue, identifier)? else {
                return Err(fail(format!(
                    "legacy goal artifact {identifier:?} is missing; refusing to discard its confirmation authority"
                )));
            };
            let folder = match state {
                agent::QueueMessageState::Pending => "inbox",
                agent::QueueMessageState::Inflight => "inflight",
                agent::QueueMessageState::Processed => "processed",
                agent::QueueMessageState::Failed => "failed",
            };
            let path = queue.join(folder).join(format!("{identifier}.json"));
            let mut document = agent::read_private_json(&path)?;
            let expected_text = goal_prompt(&record.launch.harness, objective);
            let valid = document.get("id").and_then(Value::as_str) == Some(identifier)
                && document.get("text").and_then(Value::as_str) == Some(expected_text.as_str())
                && matches!(
                    document.get("kind").and_then(Value::as_str),
                    None | Some("goal")
                );
            if !valid {
                return Err(fail(format!(
                    "legacy goal artifact {identifier:?} disagrees with its session record"
                )));
            }
            if document.get("kind").is_none() {
                document["kind"] = json!("goal");
                agent::atomic_json(&path, &document)?;
            }
        }
        Ok(())
    }

    fn goal_transaction_path(&self, record: &AgentRecord) -> Result<PathBuf> {
        Ok(self.directory(&record.name)?.join(GOAL_TRANSACTION_FILE))
    }

    fn read_goal_transaction(&self, record: &AgentRecord) -> Result<Option<(String, String)>> {
        let path = self.goal_transaction_path(record)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(fail(format!("cannot inspect goal transaction: {error}"))),
            Ok(_) => {}
        }
        let document = agent::read_private_json_bounded(&path, MAX_AGENT_RECORD_BYTES as u64)?;
        let object = document
            .as_object()
            .ok_or_else(|| fail("goal transaction is not an object"))?;
        if object.len() != 4
            || document["schema"] != GOAL_TRANSACTION_SCHEMA
            || document["token"].as_str() != Some(&record.token)
        {
            return Err(fail(
                "goal transaction is invalid or belongs to another generation",
            ));
        }
        let identifier = document["message_id"]
            .as_str()
            .ok_or_else(|| fail("goal transaction has an invalid message id"))?;
        agent::validate_message_id(identifier)?;
        let objective = document["objective"]
            .as_str()
            .filter(|value| !value.trim().is_empty() && !value.contains(['\n', '\r']))
            .ok_or_else(|| fail("goal transaction has an invalid objective"))?;
        Ok(Some((identifier.to_owned(), objective.to_owned())))
    }

    fn write_goal_transaction(
        &self,
        record: &AgentRecord,
        identifier: &str,
        objective: &str,
    ) -> Result<()> {
        let path = self.goal_transaction_path(record)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(fail(format!("cannot inspect goal transaction: {error}"))),
            Ok(_) => return Err(fail("unfinished goal transaction must be reconciled first")),
        }
        let document = json!({
            "schema": GOAL_TRANSACTION_SCHEMA,
            "token": record.token,
            "message_id": identifier,
            "objective": objective,
        });
        let encoded = serde_json::to_vec_pretty(&document)
            .map_err(|error| fail(format!("cannot encode goal transaction: {error}")))?;
        if encoded.len().saturating_add(1) > MAX_AGENT_RECORD_BYTES {
            return Err(fail("goal transaction exceeds the agent record limit"));
        }
        agent::atomic_json_create(&path, &document)
            .map_err(|error| fail(format!("cannot persist goal transaction: {error}")))
    }

    fn remove_goal_transaction(&self, record: &AgentRecord) -> Result<()> {
        let path = self.goal_transaction_path(record)?;
        fs::remove_file(&path)
            .map_err(|error| fail(format!("cannot retire goal transaction: {error}")))?;
        agent::sync_directory(
            path.parent()
                .ok_or_else(|| fail("goal transaction has no parent directory"))?,
        )
    }

    fn goal_artifact_state(
        &self,
        record: &AgentRecord,
        allow_prepared: bool,
    ) -> Result<GoalArtifactState> {
        let identifier = record
            .goal_message_id
            .as_deref()
            .ok_or_else(|| fail("goal message pointer is absent"))?;
        let objective = record
            .goal
            .as_deref()
            .ok_or_else(|| fail("goal message pointer has no session objective"))?;
        let queue = self.queue(&record.name)?;
        let state = agent::message_state(&queue, identifier)?;
        let Some(state) = state else {
            if allow_prepared
                && self.read_goal_transaction(record)?.as_ref()
                    == Some(&(identifier.to_owned(), objective.to_owned()))
            {
                return Ok(GoalArtifactState::Prepared);
            }
            return Err(fail(format!(
                "goal message {identifier:?} has no durable queue artifact"
            )));
        };
        let folder = match state {
            agent::QueueMessageState::Pending => "inbox",
            agent::QueueMessageState::Inflight => "inflight",
            agent::QueueMessageState::Processed => "processed",
            agent::QueueMessageState::Failed => "failed",
        };
        let path = queue.join(folder).join(format!("{identifier}.json"));
        let document = agent::read_private_json_bounded(&path, MAX_QUEUE_ARTIFACT_BYTES)?;
        let expected_text = goal_prompt(&record.launch.harness, objective);
        let kind_matches = document.get("kind").and_then(Value::as_str) == Some("goal")
            || (document.get("kind").is_none() && record.legacy_goal_pointer);
        if document.get("id").and_then(Value::as_str) != Some(identifier)
            || document.get("text").and_then(Value::as_str) != Some(expected_text.as_str())
            || !kind_matches
        {
            return Err(fail(format!(
                "goal message {identifier:?} disagrees with its session record"
            )));
        }
        Ok(match state {
            agent::QueueMessageState::Pending => GoalArtifactState::Pending,
            agent::QueueMessageState::Inflight => GoalArtifactState::Inflight,
            agent::QueueMessageState::Processed => GoalArtifactState::Processed,
            agent::QueueMessageState::Failed => GoalArtifactState::Failed,
        })
    }

    fn reconcile_goal_transaction(&self, record: &AgentRecord) -> Result<()> {
        let Some((identifier, objective)) = self.read_goal_transaction(record)? else {
            return Ok(());
        };
        let pointer_matches = record.goal_message_id.as_deref() == Some(&identifier);
        let state = agent::message_state(&self.queue(&record.name)?, &identifier)?;
        if !pointer_matches {
            if state.is_some() {
                return Err(fail(
                    "uncommitted goal transaction already has a queue artifact",
                ));
            }
            return self.remove_goal_transaction(record);
        }
        if record.goal.as_deref() != Some(&objective) {
            return Err(fail(
                "goal transaction disagrees with the session objective",
            ));
        }
        if state.is_none() {
            agent::enqueue_goal(
                &self.queue(&record.name)?,
                &goal_prompt(&record.launch.harness, &objective),
                &identifier,
            )?;
        }
        self.goal_artifact_state(record, false)?;
        self.remove_goal_transaction(record)
    }

    fn queue(&self, agent_name: &str) -> Result<PathBuf> {
        Ok(self.directory(agent_name)?.join("queue"))
    }

    /// Read durable metadata even if Herdr is unreachable.
    pub fn get(&self, agent_name: &str) -> Result<Value> {
        Ok(self.load(agent_name)?.public_value())
    }

    fn identity_owner(
        &self,
        session_agent: &str,
        session_value: &str,
        exclude: Option<&str>,
    ) -> Result<Option<AgentRecord>> {
        if !self.registry.exists() {
            return Ok(None);
        }
        agent::validate_private_directory(&self.registry, "agent registry", false)?;
        for entry in fs::read_dir(&self.registry).map_err(|error| fail(error.to_string()))? {
            let entry = entry.map_err(|error| fail(error.to_string()))?;
            let existing_name = entry.file_name().to_string_lossy().into_owned();
            if name(&existing_name).is_err() || Some(existing_name.as_str()) == exclude {
                continue;
            }
            let other = self.load(&existing_name)?;
            if other
                .session_agent
                .as_deref()
                .unwrap_or(&other.launch.harness)
                == session_agent
                && other.session_value.as_deref() == Some(session_value)
            {
                return Ok(Some(other));
            }
        }
        Ok(None)
    }

    /// Start one fresh tab and retain failed launch artifacts for diagnosis.
    pub fn start(&self, agent_name: &str, cwd: &Path, options: StartOptions) -> Result<Value> {
        self.start_inner(agent_name, cwd, options, None)
    }

    /// Start one fresh tab with an explicit structured reasoning effort.
    pub fn start_with_reasoning_effort(
        &self,
        agent_name: &str,
        cwd: &Path,
        reasoning_effort: &str,
        options: StartOptions,
    ) -> Result<Value> {
        self.start_inner(agent_name, cwd, options, Some(reasoning_effort))
    }

    fn start_inner(
        &self,
        agent_name: &str,
        cwd: &Path,
        mut options: StartOptions,
        reasoning_effort: Option<&str>,
    ) -> Result<Value> {
        name(agent_name)?;
        let cwd = fs::canonicalize(cwd)
            .map_err(|_| fail(format!("cwd is not a directory: {}", cwd.display())))?;
        if !cwd.is_dir() {
            return Err(fail(format!("cwd is not a directory: {}", cwd.display())));
        }
        if options.startup_timeout.is_zero() || options.startup_timeout > Duration::from_secs(300) {
            return Err(fail("startup timeout must be between 0 and 300 seconds"));
        }
        if options.brief.as_deref().is_some_and(str::is_empty) {
            return Err(fail("brief must not be empty"));
        }
        if matches!(options.harness.as_str(), "codex" | "claude" | "muse") {
            crate::profiles::validate_structured_harness_argument_conflicts(
                &format!("{} launch", options.harness),
                &options.harness,
                &options.harness_args,
                options.model.is_some(),
                reasoning_effort.is_some(),
                options.resume.is_some(),
            )?;
        }
        let mut structured_arguments =
            crate::profiles::reasoning_arguments(&options.harness, reasoning_effort)?;
        structured_arguments.extend_from_slice(&options.harness_args);
        let arguments = harness_arguments(
            &options.harness,
            options.model.as_deref(),
            options.resume.as_deref(),
            &structured_arguments,
        )?;
        options.environment = environment_entries(&options.environment)?;
        let _lock = self.lock(agent_name)?;
        let identity_lock = self.identity_lock()?;
        if let Some(resume) = options.resume.as_deref() {
            if let Some(owner) = self.identity_owner(&options.harness, resume, Some(agent_name))? {
                return Err(fail(format!(
                    "native session is already registered as {:?}",
                    owner.name
                )));
            }
        }
        let directory = self.directory(agent_name)?;
        if fs::symlink_metadata(&directory).is_ok() {
            return Err(fail(format!(
                "agent {agent_name:?} already registered; stop it before reusing the name"
            )));
        }
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&self.registry)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?;
        let mut record = AgentRecord {
            launch: LaunchSpec {
                schema: LAUNCH_SPEC_SCHEMA.to_owned(),
                harness: options.harness.clone(),
                cwd: cwd.display().to_string(),
                adapter: if options.harness == "muse" {
                    "herdr-pane".to_owned()
                } else {
                    herdr_adapter()
                },
                mode: interactive_mode(),
                backend: herdr_adapter(),
                model: options.model.clone(),
                resume: options.resume.clone(),
                profile: options.launch_profile.clone(),
                argv: std::iter::once(options.harness.clone())
                    .chain(arguments.iter().cloned())
                    .collect(),
                environment_names: options
                    .environment
                    .iter()
                    .filter_map(|entry| entry.split_once('=').map(|(name, _)| name.to_owned()))
                    .collect(),
                runtime_home: None,
                runtime_ownership: "owned".to_owned(),
                executable: None,
                permission_mode: None,
            },
            paused: false,
            pane_reported_by_agentctl: false,
            custom_process_identity: None,
            foreign_shell_identity: None,
            runner_identity: None,
            legacy_runner_pid: None,
            legacy_runner_started_at: None,
            legacy_goal_delivery: None,
            legacy_goal_messages: BTreeMap::new(),
            legacy_goal_pointer: false,
            extra: BTreeMap::new(),
            name: agent_name.to_owned(),
            token: format!("{}-{}", now.as_nanos(), std::process::id()),
            created_at: now.as_secs_f64(),
            lifecycle: "starting".to_owned(),
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            session_agent: None,
            session_value: None,
            session_source: None,
            startup_warning: None,
            effective_reasoning_effort: None,
            error: None,
            goal: None,
            goal_command: None,
            goal_message_id: None,
        };
        self.save(&record)?;
        let launched = self.launch(&mut record, &options);
        if let Err(error) = launched {
            record.lifecycle = "launch_failed".to_owned();
            record.error = Some(if options.environment.is_empty() {
                error.to_string()
            } else {
                "launch failed with caller-supplied environment; details omitted from status"
                    .to_owned()
            });
            self.save(&record)?;
            return Err(fail(format!("launch of {agent_name:?} failed: {error}; record and any created tab retained at {}", directory.display())));
        }
        drop(identity_lock);
        // Keep this generation's lifecycle lock through startup delivery and returned status.
        // Sending by name after releasing it can redirect the old brief into a replacement.
        if let Some(brief) = options.brief {
            self.send_record(&record, &brief, options.delivery, None)?;
        }
        self.status(agent_name)
    }

    /// Recover or reconcile one exactly identified live Muse process.
    pub fn recover_start(
        &self,
        agent_name: &str,
        expected_token: &str,
        expected_pid: u64,
    ) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        if record.token != expected_token {
            return Err(fail(format!(
                "agent {agent_name:?} was replaced before start recovery"
            )));
        }
        if record.lifecycle != "launch_failed"
            || record.launch.adapter != "herdr-pane"
            || record.launch.harness != "muse"
            || record.launch.mode != "interactive"
            || record.launch.backend != "herdr"
            || record.launch.runtime_ownership != "owned"
            || record.pane_id.is_none()
            || record.tab_id.is_none()
            || record.workspace_id.is_none()
            || record.launch.argv.is_empty()
            || record.session_agent.is_some()
            || record.session_value.is_some()
        {
            return Err(fail(
                "start recovery requires a launch_failed Muse interactive Herdr record with complete launch intent and pane ownership",
            ));
        }
        let pane_id = record.pane_id.clone().expect("checked pane identity");
        let presentations = self
            .client
            .panes()?
            .into_iter()
            .filter(|pane| pane.pane_id == pane_id)
            .collect::<Vec<_>>();
        if presentations.len() != 1
            || Some(&presentations[0].tab_id) != record.tab_id.as_ref()
            || Some(&presentations[0].workspace_id) != record.workspace_id.as_ref()
        {
            return Err(fail(
                "refusing start recovery: recorded pane, tab, or workspace ownership changed",
            ));
        }
        let info = self.client.pane_info(&pane_id)?;
        let cwd_matches = fs::canonicalize(&info.cwd)
            .ok()
            .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.launch.cwd).ok());
        if info.pane_id != pane_id
            || Some(&info.workspace_id) != record.workspace_id.as_ref()
            || !cwd_matches
            || info
                .agent
                .as_deref()
                .is_some_and(|agent| agent != record.launch.harness)
        {
            return Err(fail(
                "refusing start recovery: live pane identity or agent report changed",
            ));
        }
        let (mut launch_device, mut launch_inode) = record
            .launch
            .executable
            .as_ref()
            .map_or((None, None), |value| {
                (Some(value.device), Some(value.inode))
            });
        if let Some(persisted) = record.custom_process_identity.as_ref() {
            let recorded_image = (persisted.executable_device, persisted.executable_inode);
            if launch_device
                .zip(launch_inode)
                .is_some_and(|launch_image| launch_image != recorded_image)
            {
                return Err(fail(
                    "refusing start recovery: launch and process executable identities disagree",
                ));
            }
            launch_device = Some(recorded_image.0);
            launch_inode = Some(recorded_image.1);
        }
        let launch_device = launch_device.ok_or_else(|| {
            fail("start recovery requires a saved launch or process executable identity")
        })?;
        let launch_inode = launch_inode.ok_or_else(|| {
            fail("start recovery requires a saved launch or process executable identity")
        })?;
        let identity = self.client.recover_pane_agent(
            &pane_id,
            &record.launch.argv,
            launch_device,
            launch_inode,
            expected_pid,
        )?;
        if record
            .custom_process_identity
            .as_ref()
            .is_some_and(|persisted| persisted != &identity)
        {
            return Err(fail(
                "refusing start recovery: persisted custom process identity changed",
            ));
        }
        record.custom_process_identity = Some(identity.clone());
        self.save(&record)?;
        let final_check = (|| -> Result<()> {
            let screen = self.client.read(&pane_id, "visible", Some(200))?;
            if !muse_idle_composer(&screen) {
                return Err(fail(
                    "start recovery found the exact Muse process but no verified idle composer",
                ));
            }
            let harness = record.launch.harness.clone();
            let mut commit = || -> crate::error::Result<()> {
                // Custom process identity is the durable authority. Herdr's
                // native agent label remains advisory and need not be
                // published during crash recovery.
                record.lifecycle = "running".to_owned();
                record.error = None;
                self.save(&record)
                    .map_err(|error| crate::error::AdapterError::unavailable(error.to_string()))
            };
            self.client
                .commit_recovered_pane_agent(&pane_id, &harness, &identity, &mut commit)?;
            Ok(())
        })();
        if let Err(error) = final_check {
            record.lifecycle = "launch_failed".to_owned();
            record.error = Some(format!(
                "start recovery failed after identity persistence: {error}"
            ));
            self.save(&record)?;
            return Err(fail(record.error.clone().expect("saved recovery error")));
        }
        self.status(agent_name)
    }

    /// Register an identity-checked live Herdr agent without owning its runtime.
    pub fn adopt(&self, agent_name: &str, options: AdoptOptions) -> Result<Value> {
        name(agent_name)?;
        if options.pane_id.is_empty() || options.pane_id.contains('\0') {
            return Err(fail("adopt needs a nonempty pane id without NUL"));
        }
        if options.expected_workspace.is_empty() || options.expected_workspace.contains('\0') {
            return Err(fail(
                "adopt needs a nonempty expected workspace label without NUL",
            ));
        }
        harness_arguments(&options.harness, None, None, &[])?;
        if options.harness == "muse" {
            return Err(fail(
                "adopting Muse is unsupported because agentctl cannot yet pin the existing foreground process identity; start an owned Muse session instead",
            ));
        }
        let cwd = fs::canonicalize(&options.cwd)
            .map_err(|_| fail(format!("cwd is not a directory: {}", options.cwd.display())))?;
        if !cwd.is_dir() {
            return Err(fail(format!("cwd is not a directory: {}", cwd.display())));
        }
        if options
            .session
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.contains('\0'))
        {
            return Err(fail(
                "native session id must be nonempty and contain no NUL",
            ));
        }
        let target = Target {
            pane_id: Some(options.pane_id.clone()),
            session_agent: options.session.as_ref().map(|_| options.harness.clone()),
            session_value: options.session.clone(),
            expected_agent: Some(options.harness.clone()),
            expected_workspace: Some(options.expected_workspace),
            expected_cwd: Some(cwd.clone()),
        };
        let _generation_lock = self.lock(agent_name)?;
        let directory = self.directory(agent_name)?;
        if fs::symlink_metadata(&directory).is_ok() {
            return Err(fail(format!(
                "agent {agent_name:?} already registered; stop it before reusing the name"
            )));
        }
        // Different names have independent lifecycle locks. Make adoption one
        // registry-wide identity transaction so simultaneous callers cannot
        // register two panes that report the same native session.
        let _identity_lock = self.identity_lock()?;
        let (_target_lock, info) = agent::lock_resolved_target(self.client, &target)?;
        match (&info.session_agent, &info.session_value) {
            (None, None) => {}
            (Some(kind), Some(value))
                if !kind.is_empty()
                    && !value.is_empty()
                    && !kind.contains('\0')
                    && !value.contains('\0') =>
            {
                if kind != &options.harness {
                    return Err(fail(format!(
                        "refusing pane {}: native session agent is {kind:?}, expected {:?}",
                        options.pane_id, options.harness
                    )));
                }
            }
            (Some(_), Some(_)) => {
                return Err(fail(format!(
                    "refusing pane {}: native session identity is invalid",
                    options.pane_id
                )));
            }
            _ => {
                return Err(fail(format!(
                    "refusing pane {}: native session identity is incomplete",
                    options.pane_id
                )));
            }
        }
        if info.session_value.is_some() {
            // A reported native session becomes the durable queue authority.
            // Prove now that it resolves uniquely back to this exact pane.
            agent::resolve_target(
                self.client,
                &Target {
                    pane_id: Some(info.pane_id.clone()),
                    session_agent: info.session_agent.clone(),
                    session_value: info.session_value.clone(),
                    expected_agent: Some(options.harness.clone()),
                    expected_workspace: None,
                    expected_cwd: Some(cwd.clone()),
                },
            )?;
        }
        let presentations: Vec<Pane> = self
            .client
            .panes()?
            .into_iter()
            .filter(|pane| pane.pane_id == info.pane_id)
            .collect();
        if presentations.len() != 1 {
            return Err(fail(format!(
                "refusing pane {}: expected one live presentation, found {}",
                options.pane_id,
                presentations.len()
            )));
        }
        let presentation = &presentations[0];
        if presentation.workspace_id != info.workspace_id {
            return Err(fail(format!(
                "refusing pane {}: presentation workspace identity changed",
                options.pane_id
            )));
        }
        let shell_identity = self.client.pane_shell_identity(&info.pane_id)?;
        for entry in fs::read_dir(&self.registry).map_err(|error| fail(error.to_string()))? {
            let entry = entry.map_err(|error| fail(error.to_string()))?;
            let existing_name = entry.file_name().to_string_lossy().into_owned();
            if name(&existing_name).is_err() {
                continue;
            }
            let other = self.load(&existing_name)?;
            let same_session = info.session_value.is_some()
                && other
                    .session_agent
                    .as_deref()
                    .unwrap_or(&other.launch.harness)
                    == info.session_agent.as_deref().unwrap_or("")
                && other.session_value == info.session_value;
            if other.pane_id.as_ref() == Some(&info.pane_id) || same_session {
                return Err(fail(format!(
                    "pane {:?} is already registered as {:?}",
                    options.pane_id, other.name
                )));
            }
        }
        let confirmed = agent::resolve_target(
            self.client,
            &Target {
                pane_id: Some(info.pane_id.clone()),
                session_agent: info.session_agent.clone(),
                session_value: info.session_value.clone(),
                expected_agent: Some(options.harness.clone()),
                expected_workspace: None,
                expected_cwd: Some(cwd.clone()),
            },
        )?;
        if confirmed.workspace_id != info.workspace_id
            || confirmed.session_agent != info.session_agent
            || confirmed.session_value != info.session_value
        {
            return Err(fail(format!(
                "refusing pane {}: live identity changed before adoption",
                options.pane_id
            )));
        }
        let final_presentations: Vec<Pane> = self
            .client
            .panes()?
            .into_iter()
            .filter(|pane| pane.pane_id == confirmed.pane_id)
            .collect();
        if final_presentations.len() != 1
            || final_presentations[0].tab_id != presentation.tab_id
            || final_presentations[0].workspace_id != presentation.workspace_id
        {
            return Err(fail(format!(
                "refusing pane {}: live presentation changed before adoption",
                options.pane_id
            )));
        }
        if self.client.pane_shell_identity(&confirmed.pane_id)? != shell_identity {
            return Err(fail(format!(
                "refusing pane {}: pane shell process changed before adoption",
                options.pane_id
            )));
        }
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&self.registry)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?;
        let mut record = AgentRecord {
            launch: LaunchSpec {
                schema: LAUNCH_SPEC_SCHEMA.to_owned(),
                harness: options.harness,
                cwd: cwd.display().to_string(),
                adapter: "herdr-foreign".to_owned(),
                mode: interactive_mode(),
                backend: herdr_adapter(),
                model: None,
                resume: None,
                profile: None,
                argv: Vec::new(),
                environment_names: Vec::new(),
                runtime_home: None,
                runtime_ownership: "foreign".to_owned(),
                executable: None,
                permission_mode: None,
            },
            paused: false,
            pane_reported_by_agentctl: false,
            custom_process_identity: None,
            foreign_shell_identity: Some(shell_identity.clone()),
            runner_identity: None,
            legacy_runner_pid: None,
            legacy_runner_started_at: None,
            legacy_goal_delivery: None,
            legacy_goal_messages: BTreeMap::new(),
            legacy_goal_pointer: false,
            extra: BTreeMap::new(),
            name: agent_name.to_owned(),
            token: format!("{}-{}", now.as_nanos(), std::process::id()),
            created_at: now.as_secs_f64(),
            lifecycle: "running".to_owned(),
            workspace_id: Some(info.workspace_id),
            tab_id: Some(presentation.tab_id.clone()),
            pane_id: Some(info.pane_id),
            session_agent: info.session_agent.clone(),
            session_value: info.session_value.clone(),
            session_source: info.session_value.as_ref().map(|_| "observed".to_owned()),
            startup_warning: None,
            effective_reasoning_effort: None,
            error: None,
            goal: None,
            goal_command: None,
            goal_message_id: None,
        };
        self.save(&record)?;
        let final_status = self.status_record(&record).and_then(|result| {
            if !result["probe_error"].is_null() {
                return Err(fail(format!(
                    "final live-identity verification failed: {}",
                    result["probe_error"]
                )));
            }
            let final_info = self.checked(&record)?;
            if final_info.session_agent != record.session_agent
                || final_info.session_value != record.session_value
            {
                return Err(fail("final live native-session identity changed"));
            }
            let live: Vec<Pane> = self
                .client
                .panes()?
                .into_iter()
                .filter(|pane| Some(&pane.pane_id) == record.pane_id.as_ref())
                .collect();
            if live.len() != 1
                || Some(&live[0].tab_id) != record.tab_id.as_ref()
                || Some(&live[0].workspace_id) != record.workspace_id.as_ref()
            {
                return Err(fail("final live-presentation verification failed"));
            }
            if self.client.pane_shell_identity(&confirmed.pane_id)? != shell_identity {
                return Err(fail("final pane shell process identity changed"));
            }
            Ok(result)
        });
        match final_status {
            Ok(result) => Ok(result),
            Err(error) => match self.archive_failed_adoption(&mut record, &error.to_string()) {
                Ok(destination) => Err(fail(format!(
                    "adoption failed final verification and was not registered; diagnostic record archived at {}: {error}",
                    destination.display()
                ))),
                Err(cleanup) => Err(fail(format!(
                    "adoption failed final verification ({error}); could not finish failed-record archival from {}: {cleanup}",
                    directory.display()
                ))),
            },
        }
    }

    fn archive_failed_adoption(&self, record: &mut AgentRecord, error: &str) -> Result<PathBuf> {
        let archive = self.registry.join("archive");
        agent::create_private_directory(&archive, "agent archive", false, false)?;
        let destination = archive.join(format!("{}-{}-adopt-failed", record.name, record.token));
        fs::rename(self.directory(&record.name)?, &destination)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&archive)?;
        agent::sync_directory(&self.registry)?;
        record.lifecycle = "adopt_failed".to_owned();
        record.error = Some(error.to_owned());
        agent::atomic_json(&destination.join("agent.json"), &record.storage_value()?)?;
        Ok(destination)
    }

    fn launch(&self, record: &mut AgentRecord, options: &StartOptions) -> Result<()> {
        self.create_presentation(record, options)?;
        let pane_id = record.pane_id.clone().expect("new tab has pane");
        if record.launch.adapter == "herdr-pane" {
            let agent_name = record.name.clone();
            let harness = record.launch.harness.clone();
            let arguments = record.arguments().to_vec();
            self.client.start_pane_agent(
                &agent_name,
                &harness,
                &pane_id,
                &arguments,
                options.startup_timeout,
                &mut |observation| {
                    match observation {
                        CustomLaunchObservation::Intent {
                            executable,
                            device,
                            inode,
                            argv,
                        } => {
                            record.launch.executable = Some(LaunchExecutable {
                                path: executable.display().to_string(),
                                device,
                                inode,
                            });
                            record.launch.argv = argv;
                        }
                        CustomLaunchObservation::Process(identity) => {
                            record.custom_process_identity = Some(identity);
                        }
                    }
                    self.save(record).map_err(|error| {
                        crate::error::AdapterError::unavailable(format!(
                            "cannot persist custom launch identity: {error}"
                        ))
                    })
                },
            )?;
            record.pane_reported_by_agentctl = true;
            self.save(record)?;
            let screen = self.client.read(&pane_id, "visible", Some(200))?;
            (record.startup_warning, record.effective_reasoning_effort) =
                muse_startup_metadata(&screen);
        } else {
            self.client.start_agent(
                &record.name,
                &record.launch.harness,
                &pane_id,
                record.arguments(),
                options.startup_timeout,
            )?;
        }
        let info = agent::resolve_target(
            &WorkspaceClient {
                client: self.client,
                record,
                goal_objective: Mutex::new(None),
                custom_submission: Mutex::new(None),
                queue: None,
                check_prompt: true,
                adopted_evidence: Mutex::new(None),
            },
            &record.target()?,
        )?;
        record.session_agent = info.session_agent;
        record.session_value = info.session_value;
        record.session_source = record.session_value.as_ref().map(|_| "observed".to_owned());
        let owner = match (&record.session_agent, &record.session_value) {
            (Some(session_agent), Some(session_value)) => {
                self.identity_owner(session_agent, session_value, Some(&record.name))?
            }
            _ => None,
        };
        if let Some(owner) = owner {
            let pane = record.pane_id.as_deref().expect("launched pane");
            match self.client.close_pane(pane) {
                Ok(()) => {
                    record.session_agent = None;
                    record.session_value = None;
                    record.session_source = None;
                    return Err(fail(format!(
                        "native session is already registered as {:?}; closed the conflicting new pane",
                        owner.name
                    )));
                }
                Err(error) => {
                    record.session_agent = None;
                    record.session_value = None;
                    record.session_source = None;
                    return Err(fail(format!(
                        "native session is already registered as {:?}; could not close the conflicting new pane: {error}",
                        owner.name
                    )));
                }
            }
        }
        if record.session_value.is_some() {
            if let Err(error) = agent::resolve_target(self.client, &record.target()?) {
                record.session_agent = None;
                record.session_value = None;
                record.session_source = None;
                return Err(fail(format!(
                    "started native session is not globally unique; the failed owned pane remains available for stop: {error}"
                )));
            }
        }
        record.lifecycle = "running".to_owned();
        self.save(record)?;
        let final_info = match agent::resolve_target(self.client, &record.target()?)
            .and_then(|_| self.checked(record))
        {
            Ok(info) => info,
            Err(error) => {
                record.session_agent = None;
                record.session_value = None;
                record.session_source = None;
                return Err(error);
            }
        };
        if final_info.session_agent != record.session_agent
            || final_info.session_value != record.session_value
        {
            record.session_agent = None;
            record.session_value = None;
            record.session_source = None;
            return Err(fail(
                "started agent native session changed during identity commit",
            ));
        }
        Ok(())
    }

    fn create_presentation(&self, record: &mut AgentRecord, options: &StartOptions) -> Result<()> {
        if options.workspace_id.is_some() && options.workspace_label.is_some() {
            return Err(fail("workspace id and label are mutually exclusive"));
        }
        if options
            .workspace_id
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.contains('\0'))
            || options
                .workspace_label
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.contains('\0'))
        {
            return Err(fail(
                "workspace id and label must be nonempty and contain no NUL",
            ));
        }
        let environment_workspace = self.inherited_workspace.clone();
        let may_create_shared_default = options.workspace_id.is_none()
            && options.workspace_label.is_none()
            && environment_workspace.is_none();
        let mut selected_id = options.workspace_id.clone();
        let mut selected_label = options.workspace_label.clone();
        if selected_id.is_none() && selected_label.is_none() {
            selected_id = environment_workspace;
        }
        if selected_id.is_none() && selected_label.is_none() {
            selected_label = Some("subagents".to_owned());
        }
        let lock_key = selected_id
            .as_deref()
            .or(selected_label.as_deref())
            .expect("workspace selector");
        let lock = agent::open_private_lock(
            &agent::target_lock_path(&format!("managed-workspace:{lock_key}"))?,
            "workspace allocation lock",
        )?;
        lock.lock_exclusive()
            .map_err(|error| fail(error.to_string()))?;
        let selected = if let Some(workspace) = selected_id {
            self.client.workspace_label(&workspace)?;
            Some(workspace)
        } else {
            self.client
                .workspace_id_for_label(selected_label.as_deref().expect("workspace label"))?
        };
        match selected {
            Some(workspace) => {
                record.workspace_id = Some(workspace.clone());
                let (tab, pane) = self.client.create_tab_with_pane(
                    &workspace,
                    &record.name,
                    &record.launch.cwd,
                    &options.environment,
                )?;
                record.tab_id = Some(tab);
                record.pane_id = Some(pane);
                self.save(record)?;
            }
            None => {
                let label = selected_label.as_deref().expect("workspace label");
                if !may_create_shared_default {
                    return Err(fail(format!("workspace label {label:?} does not exist")));
                }
                let (workspace, tab, pane) = self.client.create_workspace(
                    label,
                    &record.launch.cwd,
                    &options.environment,
                )?;
                record.workspace_id = Some(workspace);
                record.tab_id = Some(tab.clone());
                record.pane_id = Some(pane);
                self.save(record)?;
                self.client.rename_tab(&tab, &record.name)?;
            }
        }
        Ok(())
    }

    fn checked(&self, record: &AgentRecord) -> Result<AgentPaneInfo> {
        agent::resolve_target(
            &WorkspaceClient {
                client: self.client,
                record,
                goal_objective: Mutex::new(None),
                custom_submission: Mutex::new(None),
                queue: None,
                check_prompt: false,
                adopted_evidence: Mutex::new(None),
            },
            &record.target()?,
        )
    }

    fn inspect_foreign(
        &self,
        agent_name: &str,
        record: &AgentRecord,
    ) -> Result<(bool, AgentPaneInfo, Pane)> {
        let client = WorkspaceClient {
            client: self.client,
            record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: None,
            check_prompt: false,
            adopted_evidence: Mutex::new(None),
        };
        let evidence = client.adopted_evidence_with_runtime(&agent::SystemRuntime::default(), true);
        if !matches!(
            evidence.state,
            AdoptedRuntimeState::LiveExact | AdoptedRuntimeState::IdleShellExact
        ) {
            return Err(fail(format!(
                "refusing to unregister adopted agent {agent_name:?}: {}",
                evidence.reason
            )));
        }
        Ok((
            evidence.state == AdoptedRuntimeState::LiveExact,
            evidence.info.expect("accepted evidence has pane info"),
            evidence
                .presentation
                .expect("accepted evidence has presentation"),
        ))
    }

    /// Report live state or an explicit probe error without reaping durable records.
    pub fn status(&self, agent_name: &str) -> Result<Value> {
        self.status_record(&self.load(agent_name)?)
    }

    /// Resolve and verify the exact live pane owned by one registered agent.
    pub fn pane_info(&self, agent_name: &str) -> Result<AgentPaneInfo> {
        self.checked(&self.load(agent_name)?)
    }

    /// Resolve the exact live pane while allowing a service owner to cancel control waits.
    pub(crate) fn pane_info_with_runtime(
        &self,
        agent_name: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<AgentPaneInfo> {
        let _lock = self.lock_with_runtime(agent_name, runtime)?;
        let record = self.load(agent_name)?;
        agent::resolve_target_with_runtime(
            &WorkspaceClient {
                client: self.client,
                record: &record,
                goal_objective: Mutex::new(None),
                custom_submission: Mutex::new(None),
                queue: None,
                check_prompt: false,
                adopted_evidence: Mutex::new(None),
            },
            &record.target()?,
            runtime,
        )
    }

    fn status_record(&self, record: &AgentRecord) -> Result<Value> {
        self.status_record_with_runtime(record, &agent::SystemRuntime::default())
    }

    fn status_record_with_runtime(
        &self,
        record: &AgentRecord,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<Value> {
        let agent_name = &record.name;
        let mut result = record.public_value();
        result["capabilities"] = json!(record.capabilities());
        result["queue"] = json!(self.queue(agent_name)?);
        result["output"] = json!(self.directory(agent_name)?.join("output.json"));
        result["goal_source"] = if record.goal.is_some() {
            json!("requested")
        } else {
            Value::Null
        };
        result["goal_delivery"] = json!(self.goal_delivery(record)?);
        if record.launch.adapter == "turn-runner" {
            result["agent_status"] = json!("unknown");
            result["probe_error"] = json!(
                "turn-runner liveness requires the worker extension; this edition made no death claim"
            );
            result["probe_error_kind"] = json!("unsupported-runtime-observer");
            return Ok(result);
        }
        let client = WorkspaceClient {
            client: self.client,
            record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: None,
            check_prompt: false,
            adopted_evidence: Mutex::new(None),
        };
        let probe = record.target().and_then(|target| {
            agent::status_with_runtime(&client, &target, &self.queue(agent_name)?, runtime)
        });
        match probe {
            Ok(status) => {
                result
                    .as_object_mut()
                    .expect("record object")
                    .extend(json!(status).as_object().expect("status object").clone());
                result["probe_error"] = Value::Null;
            }
            Err(error) => {
                result["agent_status"] = json!("unknown");
                result["probe_error"] = json!(error.to_string());
            }
        }
        if record.launch.adapter == "herdr-foreign" {
            let cached = {
                client
                    .adopted_evidence
                    .lock()
                    .expect("adopted evidence lock poisoned")
                    .clone()
            };
            let evidence =
                cached.unwrap_or_else(|| client.adopted_evidence_with_runtime(runtime, false));
            result["runtime_evidence"] = evidence.public_value();
        }
        Ok(result)
    }

    fn classify_health(
        &self,
        record: &AgentRecord,
        status: &Value,
        runtime: &dyn agent::AgentRuntime,
    ) -> (String, String, String) {
        if record.launch.adapter == "turn-runner" {
            return (
                "unknown".to_owned(),
                "runtime-observer-unavailable".to_owned(),
                "turn-runner liveness requires the worker extension; no death was inferred"
                    .to_owned(),
            );
        }
        if record.lifecycle != "running" {
            return (
                "unhealthy".to_owned(),
                "lifecycle-not-running".to_owned(),
                format!(
                    "saved lifecycle is {:?}, expected 'running'",
                    record.lifecycle
                ),
            );
        }
        if record.launch.adapter == "herdr-foreign" {
            let evidence = status.get("runtime_evidence");
            let state = evidence
                .and_then(|value| value.get("state"))
                .and_then(Value::as_str);
            let code = evidence
                .and_then(|value| value.get("reason_code"))
                .and_then(Value::as_str)
                .unwrap_or("runtime-probe-failed");
            let detail = evidence
                .and_then(|value| value.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("adopted-runtime probe produced no typed evidence");
            return match state {
                Some("live-exact") if status["probe_error"].is_null() => {
                    ("healthy".to_owned(), "ok".to_owned(), detail.to_owned())
                }
                Some("live-exact") => (
                    "unknown".to_owned(),
                    "runtime-probe-failed".to_owned(),
                    status["probe_error"].as_str().unwrap_or(detail).to_owned(),
                ),
                Some("unknown") => ("unknown".to_owned(), code.to_owned(), detail.to_owned()),
                Some("idle-shell-exact" | "missing" | "ambiguous" | "identity-mismatch") => {
                    ("unhealthy".to_owned(), code.to_owned(), detail.to_owned())
                }
                _ => (
                    "unknown".to_owned(),
                    "runtime-probe-failed".to_owned(),
                    "adopted-runtime probe produced malformed typed evidence".to_owned(),
                ),
            };
        }
        let Some(reason) = status.get("probe_error").and_then(Value::as_str) else {
            return (
                "healthy".to_owned(),
                "ok".to_owned(),
                "expected harness and runtime identity are live".to_owned(),
            );
        };
        let reason = reason.to_owned();
        let Some(pane_id) = record.pane_id.as_deref() else {
            return (
                "unhealthy".to_owned(),
                "pane-identity-missing".to_owned(),
                reason,
            );
        };
        let presentations = match self.client.panes_with_runtime(runtime) {
            Ok(panes) => panes
                .into_iter()
                .filter(|pane| pane.pane_id == pane_id)
                .collect::<Vec<_>>(),
            Err(_) => {
                return (
                    "unknown".to_owned(),
                    "runtime-probe-failed".to_owned(),
                    reason,
                )
            }
        };
        if presentations.is_empty() {
            return ("unhealthy".to_owned(), "pane-missing".to_owned(), reason);
        }
        if presentations.len() != 1 {
            return (
                "unhealthy".to_owned(),
                "pane-identity-ambiguous".to_owned(),
                reason,
            );
        }
        let info = match self.client.pane_info_with_runtime(pane_id, runtime) {
            Ok(info) => info,
            Err(_) => {
                return (
                    "unknown".to_owned(),
                    "runtime-probe-failed".to_owned(),
                    reason,
                )
            }
        };
        let presentation = &presentations[0];
        let session_agent_matches = record.session_agent.as_ref().is_none_or(|expected| {
            info.session_agent.as_ref() == Some(expected)
                || (info.agent.is_none() && info.session_agent.is_none())
        });
        let session_value_matches = record.session_value.as_ref().is_none_or(|expected| {
            info.session_value.as_ref() == Some(expected)
                || (info.agent.is_none() && info.session_value.is_none())
        });
        let cwd_matches = fs::canonicalize(&info.cwd)
            .ok()
            .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.launch.cwd).ok());
        if Some(&presentation.workspace_id) != record.workspace_id.as_ref()
            || Some(&presentation.tab_id) != record.tab_id.as_ref()
            || info.pane_id != pane_id
            || Some(&info.workspace_id) != record.workspace_id.as_ref()
            || !cwd_matches
            || !session_agent_matches
            || !session_value_matches
        {
            return (
                "unhealthy".to_owned(),
                "runtime-identity-mismatch".to_owned(),
                reason,
            );
        }
        if record.launch.adapter == "herdr-pane" {
            if record.custom_process_identity.is_some()
                && self
                    .client
                    .verify_custom_harness_with_runtime(
                        pane_id,
                        &record.launch.harness,
                        record.custom_process_identity.as_ref(),
                        runtime,
                    )
                    .is_ok()
            {
                return (
                    "unknown".to_owned(),
                    "custom-harness-verification-unconfirmed".to_owned(),
                    reason,
                );
            }
            let shell_fallback = match self
                .client
                .pane_idle_shell_identity_with_runtime(pane_id, runtime)
            {
                Ok(proof) => proof,
                Err(_) => {
                    return (
                        "unknown".to_owned(),
                        "runtime-probe-failed".to_owned(),
                        reason,
                    )
                }
            };
            if shell_fallback.is_some() {
                let reported = info
                    .agent
                    .as_deref()
                    .map_or_else(|| "None".to_owned(), |agent| format!("'{agent}'"));
                return (
                    "unhealthy".to_owned(),
                    "expected-harness-missing".to_owned(),
                    format!(
                        "pane '{pane_id}' has no live exact '{}' process and is at a stable idle shell (reported agent {reported}); status probe: {reason}",
                        record.launch.harness
                    ),
                );
            }
            return (
                "unknown".to_owned(),
                "custom-harness-verification-unconfirmed".to_owned(),
                reason,
            );
        }
        if info.agent.as_deref() != Some(record.launch.harness.as_str()) {
            if info.agent.is_none() {
                let shell_fallback = self
                    .client
                    .pane_idle_shell_identity_with_runtime(pane_id, runtime)
                    .ok()
                    .map(|proof| proof.is_some());
                if shell_fallback != Some(true) {
                    return (
                        "unknown".to_owned(),
                        "agent-report-missing".to_owned(),
                        reason,
                    );
                }
            }
            let reported = info
                .agent
                .as_deref()
                .map_or_else(|| "None".to_owned(), |agent| format!("'{agent}'"));
            return (
                "unhealthy".to_owned(),
                "expected-harness-missing".to_owned(),
                format!(
                    "pane '{pane_id}' reports agent {reported}, expected '{}'; status probe: {reason}",
                    record.launch.harness
                ),
            );
        }
        (
            "unknown".to_owned(),
            "runtime-probe-failed".to_owned(),
            reason,
        )
    }

    fn persist_health(
        &self,
        record: &AgentRecord,
        health: &str,
        reason_code: &str,
        reason: &str,
        checked_at: f64,
    ) -> Result<Value> {
        let path = self.directory(&record.name)?.join("health.json");
        let previous = if path.exists() {
            agent::read_private_json_bounded(&path, 64 << 10)
                .ok()
                .filter(|value| value["schema"] == HEALTH_SCHEMA)
                .unwrap_or_else(|| json!({}))
        } else {
            json!({})
        };
        let same_condition = previous["token"] == record.token
            && previous["health"] == health
            && previous["reason_code"] == reason_code
            && previous["reason"] == reason;
        let first_detected_at = if same_condition {
            previous["first_detected_at"].as_f64().unwrap_or(checked_at)
        } else {
            checked_at
        };
        let runtime_state = if health == "healthy" {
            "live"
        } else if matches!(
            reason_code,
            "expected-harness-missing" | "pane-missing" | "runner-not-live"
        ) {
            "dead"
        } else if reason_code == "lifecycle-not-running" {
            "inactive"
        } else {
            "unknown"
        };
        let mut document = json!({
            "schema": HEALTH_SCHEMA,
            "name": record.name,
            "token": record.token,
            "health": health,
            "runtime_state": runtime_state,
            "reason_code": reason_code,
            "reason": reason,
            "first_detected_at": first_detected_at,
            "last_checked_at": checked_at,
        });
        for key in [
            "last_unhealthy_at",
            "last_unhealthy_reason_code",
            "last_unhealthy_reason",
            "last_unknown_at",
            "last_unknown_reason_code",
            "last_unknown_reason",
        ] {
            if !previous[key].is_null() {
                document[key] = previous[key].clone();
            }
        }
        if health == "unhealthy" {
            document["last_unhealthy_at"] = json!(checked_at);
            document["last_unhealthy_reason_code"] = json!(reason_code);
            document["last_unhealthy_reason"] = json!(reason);
        } else if health == "unknown" {
            document["last_unknown_at"] = json!(checked_at);
            document["last_unknown_reason_code"] = json!(reason_code);
            document["last_unknown_reason"] = json!(reason);
        }
        agent::atomic_json(&path, &document)?;
        document["recorded"] = json!(true);
        document["record_path"] = json!(path);
        Ok(document)
    }

    fn health_one_result(
        &self,
        agent_name: &str,
        checked_at: f64,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<(Value, Option<Value>)> {
        if runtime.cancelled() {
            return Ok((
                Self::unrecorded_health(
                    agent_name,
                    checked_at,
                    "probe-deadline-exceeded",
                    "health probe deadline expired before the record was read",
                    None,
                ),
                None,
            ));
        }
        let initial = self.load(agent_name)?;
        if runtime.cancelled() {
            return Ok((
                Self::unrecorded_health(
                    agent_name,
                    checked_at,
                    "probe-deadline-exceeded",
                    "health probe deadline expired before this session was checked",
                    Some(&initial),
                ),
                None,
            ));
        }
        let Some(_lock) = self.try_lock_with_runtime(agent_name, runtime)? else {
            return Ok((
                Self::unrecorded_health(
                    agent_name,
                    checked_at,
                    "lifecycle-lock-contended",
                    &format!("agent {agent_name:?} lifecycle lock is held by another operation"),
                    Some(&initial),
                ),
                None,
            ));
        };
        let record = self.load(agent_name)?;
        if record.token != initial.token {
            return Err(fail(format!(
                "agent {agent_name:?} was replaced before health check"
            )));
        }
        if runtime.cancelled() {
            return Ok((
                Self::unrecorded_health(
                    agent_name,
                    checked_at,
                    "probe-deadline-exceeded",
                    "health probe deadline expired before runtime inspection",
                    Some(&record),
                ),
                None,
            ));
        }
        let status = self.status_record_with_runtime(&record, runtime)?;
        if runtime.cancelled() {
            let mut observation = Self::unrecorded_health(
                agent_name,
                checked_at,
                "probe-deadline-exceeded",
                "health probe deadline expired during runtime inspection",
                Some(&record),
            );
            observation["agent_status"] = status["agent_status"].clone();
            observation["probe_error"] = status["probe_error"].clone();
            return Ok((observation, Some(status)));
        }
        let (health, reason_code, reason) = self.classify_health(&record, &status, runtime);
        if runtime.cancelled() {
            let mut observation = Self::unrecorded_health(
                agent_name,
                checked_at,
                "probe-deadline-exceeded",
                "health probe deadline expired during liveness classification",
                Some(&record),
            );
            observation["agent_status"] = status["agent_status"].clone();
            observation["probe_error"] = status["probe_error"].clone();
            return Ok((observation, Some(status)));
        }
        let mut observation =
            self.persist_health(&record, &health, &reason_code, &reason, checked_at)?;
        observation["lifecycle"] = json!(record.lifecycle);
        observation["agent_status"] = status["agent_status"].clone();
        observation["probe_error"] = status["probe_error"].clone();
        Ok((observation, Some(status)))
    }

    fn unrecorded_health(
        agent_name: &str,
        checked_at: f64,
        reason_code: &str,
        reason: &str,
        record: Option<&AgentRecord>,
    ) -> Value {
        json!({
            "schema": HEALTH_SCHEMA,
            "name": agent_name,
            "token": record.map(|record| record.token.as_str()),
            "health": "unknown",
            "runtime_state": "unknown",
            "reason_code": reason_code,
            "reason": reason,
            "first_detected_at": checked_at,
            "last_checked_at": checked_at,
            "recorded": false,
            "record_path": Value::Null,
            "lifecycle": record.map(|record| record.lifecycle.as_str()),
            "agent_status": Value::Null,
            "probe_error": reason,
        })
    }

    fn health_one(
        &self,
        agent_name: &str,
        checked_at: f64,
        runtime: &dyn agent::AgentRuntime,
    ) -> (Value, Option<Value>) {
        self.health_one_result(agent_name, checked_at, runtime)
            .unwrap_or_else(|error| {
                (
                    Self::unrecorded_health(
                        agent_name,
                        checked_at,
                        if runtime.cancelled() {
                            "probe-deadline-exceeded"
                        } else {
                            "registry-or-health-error"
                        },
                        &error.to_string(),
                        None,
                    ),
                    None,
                )
            })
    }

    /// Return one bounded status and its health verdict from the same probe.
    pub(crate) fn status_health_snapshot(
        &self,
        agent_name: &str,
    ) -> Result<(Value, Option<Value>)> {
        let checked_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let runtime = DeadlineRuntime::until(Instant::now() + HEALTH_PROBE_TIMEOUT);
        self.health_one_result(agent_name, checked_at, &runtime)
    }

    /// Return one aggregate and the exact status snapshot behind each verdict.
    pub(crate) fn health_snapshot(&self, requested: &[String]) -> (Value, Vec<Option<Value>>) {
        self.health_snapshot_until(requested, Instant::now() + HEALTH_PROBE_TIMEOUT)
    }

    pub(crate) fn health_snapshot_until(
        &self,
        requested: &[String],
        deadline: Instant,
    ) -> (Value, Vec<Option<Value>>) {
        let checked_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let mut names = requested.to_vec();
        names.sort();
        names.dedup();
        let mut registry_error = None;
        if names.is_empty() && self.registry.exists() {
            let discovered = (|| -> Result<Vec<String>> {
                agent::validate_private_directory(&self.registry, "agent registry", false)?;
                let mut names = fs::read_dir(&self.registry)
                    .map_err(|error| fail(error.to_string()))?
                    .map(|entry| {
                        entry.map(|entry| entry.file_name().to_string_lossy().into_owned())
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|error| fail(error.to_string()))?;
                names.retain(|value| name(value).is_ok());
                names.sort();
                Ok(names)
            })();
            match discovered {
                Ok(discovered) => names = discovered,
                Err(error) => registry_error = Some(error.to_string()),
            }
        }
        let runtime = DeadlineRuntime::until(deadline);
        let checked = names
            .iter()
            .map(|name| self.health_one(name, checked_at, &runtime))
            .collect::<Vec<_>>();
        let sessions = checked
            .iter()
            .map(|(observation, _status)| observation.clone())
            .collect::<Vec<_>>();
        let statuses = checked
            .into_iter()
            .map(|(_observation, status)| status)
            .collect::<Vec<_>>();
        let healthy = registry_error.is_none()
            && sessions
                .iter()
                .all(|session| session["health"] == "healthy" && session["recorded"] == true);
        (
            json!({
                "schema": HEALTH_SCHEMA,
                "checked_at": checked_at,
                "healthy": healthy,
                "registry_error": registry_error,
                "sessions": sessions,
            }),
            statuses,
        )
    }

    /// Independently check selected names, or every active registry entry when empty.
    pub fn health(&self, requested: &[String]) -> Value {
        self.health_snapshot(requested).0
    }

    fn render_status_with_health(observation: &Value, status: Option<&Value>) -> Value {
        let mut status = status.cloned().unwrap_or_else(|| {
            json!({
                "name": observation["name"],
                "token": observation["token"],
                "lifecycle": observation["lifecycle"],
                "agent_status": observation["agent_status"],
                "probe_error": observation["probe_error"],
            })
        });
        for (destination, source) in [
            ("health", "health"),
            ("runtime_state", "runtime_state"),
            ("health_reason_code", "reason_code"),
            ("health_reason", "reason"),
            ("health_first_detected_at", "first_detected_at"),
            ("health_last_checked_at", "last_checked_at"),
            ("health_recorded", "recorded"),
            ("health_record_path", "record_path"),
        ] {
            status[destination] = observation[source].clone();
        }
        status
    }

    /// Return the canonical status payload and exit verdict from one probe.
    pub fn status_with_health(&self, agent_name: &str) -> Result<(Value, bool)> {
        let (observation, status) = self.status_health_snapshot(agent_name)?;
        let healthy = observation["health"] == "healthy";
        Ok((
            Self::render_status_with_health(&observation, status.as_ref()),
            healthy,
        ))
    }

    /// Return the canonical list payload and aggregate exit verdict.
    pub fn list_with_health(&self) -> (Vec<Value>, bool) {
        let (aggregate, statuses) = self.health_snapshot(&[]);
        let observations = aggregate["sessions"]
            .as_array()
            .expect("health snapshot always contains a session array");
        let rows = observations
            .iter()
            .zip(statuses.iter())
            .map(|(observation, status)| {
                Self::render_status_with_health(observation, status.as_ref())
            })
            .collect();
        (rows, aggregate["healthy"] == true)
    }

    /// Check selected names within one caller-owned absolute deadline.
    pub(crate) fn health_until(&self, requested: &[String], deadline: Instant) -> Value {
        self.health_snapshot_until(requested, deadline).0
    }

    /// List every registered agent, preserving records when Herdr is unavailable.
    pub fn list(&self) -> Result<Vec<Value>> {
        if !self.registry.exists() {
            return Ok(Vec::new());
        }
        agent::validate_private_directory(&self.registry, "agent registry", false)?;
        let mut names = fs::read_dir(&self.registry)
            .map_err(|error| fail(error.to_string()))?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| fail(error.to_string()))?;
        names.retain(|value| name(value).is_ok());
        names.sort();
        names.iter().map(|name| self.status(name)).collect()
    }

    /// Serialize against stop and durably deliver one prompt.
    pub fn send(&self, agent_name: &str, text: &str, options: DrainOptions) -> Result<QueueResult> {
        self.send_identified(agent_name, text, options, None)
    }

    /// Submit a prompt with an optional caller-supplied ID; existing IDs are refused.
    pub fn send_identified(
        &self,
        agent_name: &str,
        text: &str,
        options: DrainOptions,
        message_id: Option<&str>,
    ) -> Result<QueueResult> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        self.reconcile_goal_transaction(&record)?;
        self.send_record(&record, text, options, message_id)
    }

    /// Submit through an injected runtime that can interrupt bounded readiness waits.
    pub(crate) fn send_identified_with_runtime(
        &self,
        agent_name: &str,
        text: &str,
        options: DrainOptions,
        message_id: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<QueueResult> {
        let _lock = self.lock_with_runtime(agent_name, runtime)?;
        let record = self.load(agent_name)?;
        self.reconcile_goal_transaction(&record)?;
        record.input_allowed()?;
        if !record.legacy_goal_messages.is_empty() {
            self.save(&record)?;
        }
        let queue = self.queue(&record.name)?;
        let client = WorkspaceClient {
            client: self.client,
            record: &record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: Some(&queue),
            check_prompt: true,
            adopted_evidence: Mutex::new(None),
        };
        agent::send_identified_with_runtime(
            &client,
            &record.target()?,
            &queue,
            text,
            options,
            runtime,
            Some(message_id),
        )
    }

    /// Reconcile one caller-selected durable delivery identifier by exact path lookup.
    pub fn message_state(
        &self,
        agent_name: &str,
        message_id: &str,
    ) -> Result<Option<agent::QueueMessageState>> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        record.supported()?;
        agent::message_state(&self.queue(agent_name)?, message_id)
    }

    pub(crate) fn message_state_with_runtime(
        &self,
        agent_name: &str,
        message_id: &str,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<Option<agent::QueueMessageState>> {
        let _lock = self.lock_with_runtime(agent_name, runtime)?;
        let record = self.load(agent_name)?;
        record.supported()?;
        agent::message_state(&self.queue(agent_name)?, message_id)
    }

    fn send_record(
        &self,
        record: &AgentRecord,
        text: &str,
        options: DrainOptions,
        message_id: Option<&str>,
    ) -> Result<QueueResult> {
        record.input_allowed()?;
        if !record.legacy_goal_messages.is_empty() {
            self.save(record)?;
        }
        let queue = self.queue(&record.name)?;
        let client = WorkspaceClient {
            client: self.client,
            record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: Some(&queue),
            check_prompt: true,
            adopted_evidence: Mutex::new(None),
        };
        match message_id {
            Some(identifier) => agent::send_identified(
                &client,
                &record.target()?,
                &queue,
                text,
                identifier,
                options,
            ),
            None => agent::send(&client, &record.target()?, &queue, text, options),
        }
    }

    /// Hand input ownership to a human, without signaling or suspending the harness.
    pub fn pause(&self, agent_name: &str, paused: bool) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        self.reconcile_goal_transaction(&record)?;
        record.supported()?;
        self.checked(&record)?;
        record.paused = paused;
        self.save(&record)?;
        Ok(json!({"name": agent_name, "token": record.token, "paused": paused}))
    }

    fn read_relocation_journal(&self, record: &AgentRecord) -> Result<Option<RelocationJournal>> {
        let path = self.directory(&record.name)?.join("relocation.json");
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(fail(format!("cannot inspect relocation journal: {error}"))),
        }
        let journal: RelocationJournal = serde_json::from_value(agent::read_private_json_bounded(
            &path,
            MAX_AGENT_RECORD_BYTES as u64,
        )?)
        .map_err(|error| fail(format!("invalid relocation journal: {error}")))?;
        journal.validate(&record.token)?;
        Ok(Some(journal))
    }

    fn finish_relocation(
        &self,
        record: &AgentRecord,
        journal: &RelocationJournal,
        pane: &Pane,
    ) -> Result<Value> {
        if pane.terminal_id.as_deref() != Some(&journal.terminal_id)
            || pane.workspace_id != journal.target_workspace_id
        {
            return Err(fail("relocated pane identity or workspace changed"));
        }
        let panes = self.client.panes()?;
        if panes
            .iter()
            .filter(|item| item.tab_id == pane.tab_id)
            .count()
            != 1
        {
            return Err(fail("relocated destination tab is not a one-pane tab"));
        }
        let mut candidate = record.clone();
        candidate.workspace_id = Some(pane.workspace_id.clone());
        candidate.tab_id = Some(pane.tab_id.clone());
        candidate.pane_id = Some(pane.pane_id.clone());
        self.checked(&candidate)?;
        agent::relocate_existing_binding(
            &self.queue(&record.name)?,
            &record.target()?,
            &candidate.target()?,
        )?;
        self.save(&candidate)?;
        let persisted = self.load(&record.name)?;
        if persisted.token != record.token
            || persisted.workspace_id.as_deref() != Some(&pane.workspace_id)
            || persisted.tab_id.as_deref() != Some(&pane.tab_id)
            || persisted.pane_id.as_deref() != Some(&pane.pane_id)
        {
            return Err(fail("relocation routing commit did not persist"));
        }
        self.checked(&persisted)?;
        agent::validate_existing_binding(&self.queue(&record.name)?, &persisted.target()?)?;
        let current = self
            .client
            .panes()?
            .into_iter()
            .filter(|item| item.terminal_id.as_deref() == Some(&journal.terminal_id))
            .collect::<Vec<_>>();
        if current.as_slice() != [pane.clone()] {
            return Err(fail("relocated terminal changed after routing commit"));
        }
        let pinned = self.pinned_agent_directory(&record.name)?;
        Self::verify_pinned_agent_directory(&pinned)?;
        Self::unlink_pinned_file(&pinned, "relocation.json")?;
        Ok(json!({
            "name": persisted.name,
            "token": persisted.token,
            "workspace_id": pane.workspace_id,
            "tab_id": pane.tab_id,
            "pane_id": pane.pane_id,
            "terminal_id": pane.terminal_id,
            "relocated": true,
        }))
    }

    /// Move one live interactive pane through a recoverable routing transaction.
    pub fn relocate(
        &self,
        agent_name: &str,
        workspace_id: Option<&str>,
        workspace_label: Option<&str>,
        new_tab: bool,
    ) -> Result<Value> {
        if !new_tab {
            return Err(fail("relocate currently requires --new-tab"));
        }
        if workspace_id.is_some() == workspace_label.is_some() {
            return Err(fail(
                "relocate requires exactly one of workspace id or workspace label",
            ));
        }
        let selector = workspace_id
            .or(workspace_label)
            .expect("workspace selector");
        if selector.is_empty() || selector.contains('\0') {
            return Err(fail(
                "workspace selector must be nonempty and contain no NUL",
            ));
        }
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        self.reconcile_goal_transaction(&record)?;
        if record.lifecycle != "running"
            || record.launch.mode != "interactive"
            || record.launch.backend != "herdr"
            || record.workspace_id.is_none()
            || record.tab_id.is_none()
            || record.pane_id.is_none()
        {
            return Err(fail(
                "relocate requires a running interactive Herdr session with complete routing",
            ));
        }
        let target_workspace = if let Some(workspace) = workspace_id {
            self.client.workspace_label(workspace)?;
            workspace.to_owned()
        } else {
            self.client
                .workspace_id_for_label(workspace_label.expect("workspace label"))?
                .ok_or_else(|| {
                    fail(format!(
                        "workspace label {:?} does not exist",
                        workspace_label.expect("workspace label")
                    ))
                })?
        };
        let mut journal = self.read_relocation_journal(&record)?;
        if journal
            .as_ref()
            .is_some_and(|value| value.target_workspace_id != target_workspace)
        {
            return Err(fail(
                "unfinished relocation targets another workspace; retry that exact target",
            ));
        }
        if journal.is_none() {
            if record.workspace_id.as_deref() == Some(&target_workspace) {
                return Err(fail("agent is already in the requested workspace"));
            }
            self.checked(&record)?;
            let panes = self.client.panes()?;
            let old = panes
                .iter()
                .filter(|pane| record.pane_id.as_deref() == Some(&pane.pane_id))
                .collect::<Vec<_>>();
            if old.len() != 1
                || record.tab_id.as_deref() != Some(&old[0].tab_id)
                || record.workspace_id.as_deref() != Some(&old[0].workspace_id)
                || old[0].terminal_id.is_none()
            {
                return Err(fail(
                    "cannot prove the current pane/tab/terminal routing for relocation",
                ));
            }
            if panes
                .iter()
                .filter(|pane| record.tab_id.as_deref() == Some(&pane.tab_id))
                .count()
                != 1
            {
                return Err(fail("relocate refuses a multi-pane source tab"));
            }
            let prepared = RelocationJournal {
                schema: RELOCATION_SCHEMA.to_owned(),
                token: record.token.clone(),
                terminal_id: old[0].terminal_id.clone().expect("terminal id"),
                old: PaneRoute::from_pane(old[0]),
                target_workspace_id: target_workspace.clone(),
                new_tab: true,
            };
            agent::atomic_json(
                &self.directory(agent_name)?.join("relocation.json"),
                &serde_json::to_value(&prepared)
                    .map_err(|error| fail(format!("cannot encode relocation journal: {error}")))?,
            )?;
            journal = Some(prepared);
        }
        let journal = journal.expect("prepared relocation");
        let panes = self.client.panes()?;
        let current = panes
            .iter()
            .filter(|pane| pane.terminal_id.as_deref() == Some(&journal.terminal_id))
            .collect::<Vec<_>>();
        if current.len() != 1 {
            return Err(fail(
                "cannot uniquely locate the relocation terminal generation",
            ));
        }
        let mut pane = current[0].clone();
        if PaneRoute::from_pane(&pane) == journal.old {
            self.checked(&record)?;
            let fresh_panes = self.client.panes()?;
            let fresh = fresh_panes
                .iter()
                .filter(|item| item.terminal_id.as_deref() == Some(&journal.terminal_id))
                .collect::<Vec<_>>();
            if fresh.len() != 1
                || PaneRoute::from_pane(fresh[0]) != journal.old
                || fresh_panes
                    .iter()
                    .filter(|item| item.tab_id == journal.old.tab_id)
                    .count()
                    != 1
            {
                return Err(fail("source pane routing changed before relocation"));
            }
            pane = fresh[0].clone();
            let moved = self.client.move_pane_to_new_tab(
                &pane.pane_id,
                &journal.terminal_id,
                &target_workspace,
                &record.name,
            )?;
            if (
                moved.previous_pane_id,
                moved.previous_tab_id,
                moved.previous_workspace_id,
            ) != (
                journal.old.pane_id.clone(),
                journal.old.tab_id.clone(),
                journal.old.workspace_id.clone(),
            ) {
                return Err(fail(
                    "pane move returned a different source routing identity",
                ));
            }
            pane = moved.pane;
        } else if pane.workspace_id != target_workspace {
            return Err(fail(
                "relocation terminal is neither at its old route nor intended destination",
            ));
        }
        self.finish_relocation(&record, &journal, &pane)
    }

    /// Verify the live agent identity, then focus its pane without changing input ownership.
    pub fn attach(&self, agent_name: &str) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        let info = self.checked(&record)?;
        self.client.focus_pane(&info.pane_id)?;
        Ok(json!({"name": agent_name, "pane_id": info.pane_id, "paused": record.paused}))
    }

    /// Drain only prompts known not to have been injected.
    pub fn drain(&self, agent_name: &str, options: DrainOptions) -> Result<QueueResult> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        self.reconcile_goal_transaction(&record)?;
        record.input_allowed()?;
        if !record.legacy_goal_messages.is_empty() {
            self.save(&record)?;
        }
        agent::drain(
            &WorkspaceClient {
                client: self.client,
                record: &record,
                goal_objective: Mutex::new(None),
                custom_submission: Mutex::new(None),
                queue: Some(&self.queue(agent_name)?),
                check_prompt: true,
                adopted_evidence: Mutex::new(None),
            },
            &record.target()?,
            &self.queue(agent_name)?,
            options,
        )
    }

    /// Reconcile one ambiguous Muse prompt from exact live user-turn evidence.
    pub fn reconcile_delivery(
        &self,
        agent_name: &str,
        message_id: &str,
        expected_sha256: &str,
    ) -> Result<QueueResult> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        record.supported()?;
        if record.launch.adapter != "herdr-pane" || record.launch.harness != "muse" {
            return Err(fail(
                "delivery reconciliation currently requires an owned interactive Muse pane",
            ));
        }
        let queue = self.queue(agent_name)?;
        let client = WorkspaceClient {
            client: self.client,
            record: &record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: Some(&queue),
            check_prompt: true,
            adopted_evidence: Mutex::new(None),
        };
        let target = record.target()?;
        if fs::symlink_metadata(queue.join("target.json")).is_err() {
            return Err(fail(
                "delivery reconciliation requires an existing exact queue binding",
            ));
        }
        agent::validate_existing_binding(&queue, &target)?;
        agent::reconcile_delivery(&queue, message_id, expected_sha256, |text| {
            let (_target_lock, before) = agent::lock_resolved_target(&client, &target)?;
            let screen = client.read(&before.pane_id, "recent-unwrapped", Some(5000))?;
            let after = agent::resolve_target(&client, &target)?;
            Ok(before == after
                && muse_verified_process_prompt_transcript_count(&screen, text) > 0
                && !muse_verified_process_prompt_in_composer(&screen, text))
        })
    }

    /// Drain through an injected runtime that can interrupt bounded readiness waits.
    pub(crate) fn drain_with_runtime(
        &self,
        agent_name: &str,
        options: DrainOptions,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<QueueResult> {
        let _lock = self.lock_with_runtime(agent_name, runtime)?;
        let record = self.load(agent_name)?;
        record.input_allowed()?;
        if !record.legacy_goal_messages.is_empty() {
            self.save(&record)?;
        }
        let queue = self.queue(&record.name)?;
        agent::drain_with_runtime(
            &WorkspaceClient {
                client: self.client,
                record: &record,
                goal_objective: Mutex::new(None),
                custom_submission: Mutex::new(None),
                queue: Some(&queue),
                check_prompt: true,
                adopted_evidence: Mutex::new(None),
            },
            &record.target()?,
            &queue,
            options,
            runtime,
        )
    }

    fn snapshot(&self, record: &AgentRecord, text: &str) -> Result<()> {
        let captured = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_secs_f64();
        agent::atomic_json(
            &self.directory(&record.name)?.join("output.json"),
            &json!({"text":text,"captured_at":captured,"pane_id":record.pane_id}),
        )
    }

    /// Read the shared terminal and persist the latest bounded snapshot.
    pub fn read(&self, agent_name: &str, lines: usize) -> Result<String> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        self.checked(&record)?;
        let text = agent::read(self.client, &record.target()?, lines)?;
        self.snapshot(&record, &text)?;
        Ok(text)
    }

    /// Read and persist a snapshot while allowing service cancellation during locks and control.
    pub(crate) fn read_with_runtime(
        &self,
        agent_name: &str,
        lines: usize,
        runtime: &dyn agent::AgentRuntime,
    ) -> Result<String> {
        let _lock = self.lock_with_runtime(agent_name, runtime)?;
        let record = self.load(agent_name)?;
        let target = record.target()?;
        let info = agent::resolve_target_with_runtime(
            &WorkspaceClient {
                client: self.client,
                record: &record,
                goal_objective: Mutex::new(None),
                custom_submission: Mutex::new(None),
                queue: None,
                check_prompt: false,
                adopted_evidence: Mutex::new(None),
            },
            &target,
            runtime,
        )?;
        let text = self
            .client
            .read_with_runtime(&info.pane_id, "recent", Some(lines), runtime)?;
        self.snapshot(&record, &text)?;
        Ok(text)
    }

    /// Wait for readiness, which is separate from completion of the agent's goal.
    pub fn wait(&self, agent_name: &str, timeout: Duration) -> Result<Value> {
        if timeout > Duration::from_secs(31_536_000) {
            return Err(fail(
                "wait timeout must be finite and between 0 and 31536000 seconds",
            ));
        }
        let token = self.load(agent_name)?.token;
        let start = Instant::now();
        loop {
            let lock = self.lock(agent_name)?;
            let record = self.load(agent_name)?;
            if record.token != token {
                return Err(fail(format!(
                    "agent {agent_name:?} was replaced while waiting"
                )));
            }
            let info = self.checked(&record)?;
            if matches!(info.status.as_str(), "idle" | "done" | "staged") {
                return self.status_record(&record);
            }
            if !matches!(info.status.as_str(), "working" | "starting" | "unknown") {
                return Err(fail(format!(
                    "agent {agent_name:?} requires attention (state {:?}); read its pane",
                    info.status
                )));
            }
            if start.elapsed() >= timeout {
                return Err(fail(format!(
                    "timed out waiting for agent {agent_name:?} (state {:?})",
                    info.status
                )));
            }
            drop(lock);
            std::thread::sleep(
                Duration::from_millis(250).min(timeout.saturating_sub(start.elapsed())),
            );
        }
    }

    /// Bind an explicitly known native session without changing the existing queue identity.
    pub fn bind_session(
        &self,
        agent_name: &str,
        session_id: &str,
        goal_command: Option<&[String]>,
    ) -> Result<Value> {
        if session_id.is_empty() || session_id.contains('\0') {
            return Err(fail(
                "native session id must be nonempty and contain no NUL",
            ));
        }
        if goal_command.is_some_and(|command| {
            command.is_empty()
                || command
                    .iter()
                    .any(|item| item.is_empty() || item.contains('\0'))
        }) {
            return Err(fail("goal command must be a nonempty argument vector"));
        }
        let _lock = self.lock(agent_name)?;
        let _identity_lock = self.identity_lock()?;
        let mut record = self.load(agent_name)?;
        self.reconcile_goal_transaction(&record)?;
        let info = self.checked(&record)?;
        for existing in [&record.session_value, &info.session_value]
            .into_iter()
            .flatten()
        {
            if existing != session_id {
                return Err(fail("refusing to replace an already bound native session"));
            }
        }
        if let Some(owner) =
            self.identity_owner(&record.launch.harness, session_id, Some(agent_name))?
        {
            return Err(fail(format!(
                "native session is already registered as {:?}",
                owner.name
            )));
        }
        let mut reported = Vec::new();
        for pane in self.client.panes()? {
            let live = self.client.pane_info(&pane.pane_id)?;
            if live.session_agent.as_deref() == Some(&record.launch.harness)
                && live.session_value.as_deref() == Some(session_id)
            {
                reported.push(pane.pane_id);
            }
        }
        if !reported.is_empty()
            && (reported.len() != 1 || Some(&reported[0]) != record.pane_id.as_ref())
        {
            return Err(fail(
                "native session is reported by another or ambiguous live pane",
            ));
        }
        // Session metadata is optional in Herdr. This is an explicit caller
        // assertion anchored to the independently verified live name and pane.
        record.session_agent = Some(record.launch.harness.clone());
        record.session_value = Some(session_id.to_owned());
        record.session_source = Some(
            if info.session_agent.as_deref() == Some(&record.launch.harness)
                && info.session_value.as_deref() == Some(session_id)
            {
                "observed"
            } else {
                "asserted"
            }
            .to_owned(),
        );
        if let Some(command) = goal_command {
            record.goal_command = Some(command.to_vec());
        }
        self.save(&record)?;
        Ok(json!({"name":agent_name,"session_id":session_id,"source":"explicit"}))
    }

    fn goal_delivery(&self, record: &AgentRecord) -> Result<Option<String>> {
        if record.goal_message_id.is_some() {
            return Ok(Some(
                match self.goal_artifact_state(record, true)? {
                    GoalArtifactState::Prepared | GoalArtifactState::Pending => "pending",
                    GoalArtifactState::Inflight | GoalArtifactState::Failed => "possibly_submitted",
                    GoalArtifactState::Processed => "delivered",
                }
                .to_owned(),
            ));
        }
        // Decode-edge compatibility for rows written before session/v3.
        Ok(record.legacy_goal_delivery.clone())
    }

    fn goal_result(&self, record: &AgentRecord, command: Option<&[String]>) -> Result<Value> {
        let mut result = json!({"name":record.name,"goal":record.goal,"delivery":self.goal_delivery(record)?,"source":"requested","native_status":"unverified"});
        if record.launch.harness != "codex" {
            return Ok(result);
        }
        let Some(session) = record.session_value.as_deref().filter(|s| !s.is_empty()) else {
            result["native_error"] = json!(
                "native session unknown; bind-session with the session id reported by this agent"
            );
            return Ok(result);
        };
        let default = [
            "codex".to_owned(),
            "app-server".to_owned(),
            "proxy".to_owned(),
        ];
        let command = command
            .or(record.goal_command.as_deref())
            .unwrap_or(&default);
        match crate::codex_goal::get_goal(session, command, Duration::from_secs(30)) {
            Ok(native) => {
                result["source"] = json!("native");
                result["native_status"] = native
                    .as_ref()
                    .map_or(json!("absent"), |goal| goal["status"].clone());
                result["goal"] = native
                    .as_ref()
                    .map_or(Value::Null, |goal| goal["objective"].clone());
                result["native"] = json!(native);
            }
            Err(error) => result["native_error"] = json!(error),
        }
        Ok(result)
    }

    /// Inspect a native Codex goal when bound, or submit a visible goal and wake its harness.
    pub fn goal(
        &self,
        agent_name: &str,
        text: Option<&str>,
        options: DrainOptions,
        goal_command: Option<&[String]>,
    ) -> Result<Value> {
        let Some(text) = text else {
            let record = self.load(agent_name)?;
            self.checked(&record)?;
            return self.goal_result(&record, goal_command);
        };
        if text.trim().is_empty() || text.contains(['\n', '\r']) {
            return Err(fail("goal must be a nonempty single line"));
        }
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        self.reconcile_goal_transaction(&record)?;
        record.input_allowed()?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_nanos();
        let identifier = format!("{timestamp:020}-{}", std::process::id());
        self.write_goal_transaction(&record, &identifier, text)?;
        record.goal = Some(text.to_owned());
        record.goal_message_id = Some(identifier.clone());
        self.save(&record)?;
        let prompt = goal_prompt(&record.launch.harness, text);
        let queue = self.queue(agent_name)?;
        agent::enqueue_goal(&queue, &prompt, &identifier)?;
        self.remove_goal_transaction(&record)?;
        let client = WorkspaceClient {
            client: self.client,
            record: &record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: Some(&queue),
            check_prompt: true,
            adopted_evidence: Mutex::new(None),
        };
        let outcome =
            agent::drain(&client, &record.target()?, &queue, options).and_then(|drained| {
                agent::finish_identified_delivery(&queue, identifier.clone(), drained)
            });
        match outcome {
            Ok(_) => self.goal_result(&record, goal_command),
            Err(error) => Err(error),
        }
    }

    fn checked_or_launch_failed(&self, record: &AgentRecord, pane: &str) -> Result<()> {
        if record.launch.adapter != "herdr-pane" {
            if record.lifecycle != "launch_failed" || self.client.pane_info(pane)?.agent.is_some() {
                self.checked(record)?;
            }
            return Ok(());
        }
        if matches!(record.lifecycle.as_str(), "starting" | "launch_failed")
            && record.custom_process_identity.is_some()
        {
            let info = self.client.pane_info(pane)?;
            let cwd_matches = info.cwd == record.launch.cwd
                || fs::canonicalize(&info.cwd)
                    .ok()
                    .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.launch.cwd).ok());
            if info.pane_id == pane
                && Some(&info.workspace_id) == record.workspace_id.as_ref()
                && cwd_matches
                && self
                    .client
                    .verify_custom_harness(
                        pane,
                        &record.launch.harness,
                        record.custom_process_identity.as_ref(),
                    )
                    .is_ok()
            {
                return Ok(());
            }
        }
        if record.lifecycle == "launch_failed" && record.pane_reported_by_agentctl {
            let info = self.client.pane_info(pane)?;
            let cwd_matches = info.cwd == record.launch.cwd
                || fs::canonicalize(&info.cwd)
                    .ok()
                    .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.launch.cwd).ok());
            if info.pane_id == pane
                && Some(&info.workspace_id) == record.workspace_id.as_ref()
                && cwd_matches
                && info.agent.as_deref() == Some(&record.launch.harness)
                && self.client.pane_is_idle_shell(pane)?
            {
                return Ok(());
            }
        }
        if record.lifecycle == "starting" && record.custom_process_identity.is_none() {
            return Err(fail(format!(
                "cannot prove starting custom harness ownership in pane {pane}"
            )));
        }
        self.checked(record)?;
        Ok(())
    }

    fn dead_pane_snapshot(
        &self,
        record: &AgentRecord,
        operation: &str,
    ) -> Result<(Pane, AgentPaneInfo)> {
        let pane_id = record.pane_id.as_deref().ok_or_else(|| {
            fail(format!(
                "refusing to {operation} {:?}: record lacks pane identity",
                record.name
            ))
        })?;
        let tab_id = record.tab_id.as_deref().ok_or_else(|| {
            fail(format!(
                "refusing to {operation} {:?}: record lacks tab identity",
                record.name
            ))
        })?;
        let workspace_id = record.workspace_id.as_deref().ok_or_else(|| {
            fail(format!(
                "refusing to {operation} {:?}: record lacks workspace identity",
                record.name
            ))
        })?;
        let panes = self.client.panes()?;
        let mut presentations: Vec<Pane> = panes
            .iter()
            .filter(|pane| pane.pane_id == pane_id)
            .cloned()
            .collect();
        if presentations.len() != 1 {
            return Err(fail(format!(
                "refusing to {operation} {:?}: expected one recorded pane, found {}",
                record.name,
                presentations.len()
            )));
        }
        let presentation = presentations.pop().expect("one presentation was checked");
        if presentation.tab_id != tab_id || presentation.workspace_id != workspace_id {
            return Err(fail(format!(
                "refusing to {operation} {:?}: recorded pane, tab, or workspace changed",
                record.name
            )));
        }
        let tab_panes: Vec<&Pane> = panes.iter().filter(|pane| pane.tab_id == tab_id).collect();
        if tab_panes.len() != 1 || *tab_panes[0] != presentation {
            return Err(fail(format!(
                "refusing to {operation} {:?}: recorded tab is not the exact one-pane tab",
                record.name
            )));
        }
        let info = self.client.pane_info(pane_id)?;
        let cwd_matches = info.cwd == record.launch.cwd
            || fs::canonicalize(&info.cwd)
                .ok()
                .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.launch.cwd).ok());
        if info.pane_id != pane_id || info.workspace_id != workspace_id || !cwd_matches {
            return Err(fail(format!(
                "refusing to {operation} {:?}: recorded pane, workspace, or cwd changed",
                record.name
            )));
        }
        let stale_custom_label = record.launch.adapter == "herdr-pane"
            && record.custom_process_identity.is_some()
            && info.agent.as_deref() == Some(&record.launch.harness);
        if info.agent.is_some() && !stale_custom_label {
            let agent = info.agent.as_deref().expect("checked present agent");
            return Err(fail(format!(
                "refusing to {operation} {:?}: pane still reports agent {agent:?}",
                record.name
            )));
        }
        if info.session_agent.is_some() || info.session_value.is_some() {
            return Err(fail(format!(
                "refusing to {operation} {:?}: absent agent has native session identity",
                record.name
            )));
        }
        Ok((presentation, info))
    }

    fn dead_pane_proof(&self, record: &AgentRecord, operation: &str) -> Result<DeadPaneProof> {
        let pane_id = record.pane_id.as_deref().ok_or_else(|| {
            fail(format!(
                "refusing to {operation} {:?}: record lacks pane identity",
                record.name
            ))
        })?;
        let (presentation, info) = self.dead_pane_snapshot(record, operation)?;
        if record.launch.adapter == "herdr-pane" {
            let identity = record.custom_process_identity.as_ref().ok_or_else(|| {
                fail(format!(
                    "refusing to {operation} {:?}: record lacks custom process identity",
                    record.name
                ))
            })?;
            if !self.client.process_generation_absent(identity).map_err(|error| {
                fail(format!(
                    "refusing to {operation} {:?}: cannot prove recorded custom process generation absent: {error}",
                    record.name
                ))
            })? {
                return Err(fail(format!(
                    "refusing to {operation} {:?}: recorded custom process generation is still live",
                    record.name
                )));
            }
        }
        let shell = self
            .client
            .pane_idle_shell_identity(pane_id)
            .map_err(|error| {
                fail(format!(
                    "refusing to {operation} {:?}: cannot prove supported idle shell generation: {error}",
                    record.name
                ))
            })?
            .ok_or_else(|| {
                fail(format!(
                    "refusing to {operation} {:?}: pane is not a supported idle shell generation without descendants",
                    record.name
                ))
            })?;
        let (final_presentation, final_info) = self.dead_pane_snapshot(record, operation)?;
        if final_presentation != presentation || final_info != info {
            return Err(fail(format!(
                "refusing to {operation} {:?}: pane membership or agent state changed during shell proof",
                record.name
            )));
        }
        Ok(DeadPaneProof {
            info: final_info,
            presentation: final_presentation,
            shell,
        })
    }

    fn bounded_terminal_text(&self, pane_id: &str) -> Result<String> {
        let mut text = self
            .client
            .read(pane_id, "recent-unwrapped", Some(5000))
            .map_err(|error| {
                fail(format!(
                    "cannot preserve terminal output before stop: {error}"
                ))
            })?;
        if text.is_empty() {
            text = self
                .client
                .read(pane_id, "recent", Some(5000))
                .map_err(|error| {
                    fail(format!(
                        "cannot preserve terminal output before stop: {error}"
                    ))
                })?;
        }
        Ok(text)
    }

    fn shell_proof_json(proof: &PaneShellProof) -> Value {
        json!({
            "version": proof.identity.version,
            "boot_id": proof.identity.boot_id,
            "pid": proof.identity.pid,
            "starttime_ticks": proof.identity.starttime_ticks,
            "executable_device": proof.identity.executable_device,
            "executable_inode": proof.identity.executable_inode,
            "executable_path": proof.executable_path,
        })
    }

    fn archive_destination(&self, record: &AgentRecord) -> Result<(PathBuf, PathBuf)> {
        let archive = self.registry.join("archive");
        agent::create_private_directory(&archive, "agent archive", false, false)?;
        let destination = archive.join(format!("{}-{}", record.name, record.token));
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                return Err(fail(format!(
                    "refusing to overwrite existing agent archive {}",
                    destination.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(fail(error.to_string())),
        }
        Ok((archive, destination))
    }

    fn publish_pinned_directory(
        &self,
        pinned: &PinnedAgentDirectory,
        destination: &Path,
        expected_record: &[u8],
    ) -> Result<()> {
        let mut published = false;
        self.publish_pinned_directory_with(
            pinned,
            destination,
            expected_record,
            &mut published,
            (
                rename_directory_noreplace_at,
                || {},
                || {},
                |directory: &File, _label| directory.sync_all(),
            ),
        )
    }

    fn publish_pinned_directory_with<Rename, AfterRename, AfterRollback, Sync>(
        &self,
        pinned: &PinnedAgentDirectory,
        destination: &Path,
        expected_record: &[u8],
        published: &mut bool,
        publication_hooks: (Rename, AfterRename, AfterRollback, Sync),
    ) -> Result<()>
    where
        Rename: FnOnce(&File, &str, &File, &str) -> Result<()>,
        AfterRename: FnOnce(),
        AfterRollback: FnOnce(),
        Sync: FnMut(&File, &str) -> io::Result<()>,
    {
        let (rename, after_rename, after_rollback, mut sync) = publication_hooks;
        *published = false;
        let archive_path = destination
            .parent()
            .ok_or_else(|| fail("agent archive destination has no parent"))?;
        if archive_path != self.registry.join("archive") {
            return Err(fail("invalid agent archive destination"));
        }
        let destination_name = destination
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| fail("invalid agent archive destination name"))?;
        let registry_parent = Self::pinned_parent_directory(&self.registry, "agent registry")?;
        let archive_parent = Self::pinned_parent_directory(archive_path, "agent archive")?;
        Self::verify_pinned_agent_directory(pinned)?;
        if Self::child_directory_identity(&registry_parent.file, &pinned.name)?
            != Some((pinned.device, pinned.inode))
        {
            return Err(fail(format!(
                "agent {:?} registry directory changed before publication",
                pinned.name
            )));
        }
        if let Err(rename_error) = rename(
            &registry_parent.file,
            &pinned.name,
            &archive_parent.file,
            destination_name,
        ) {
            let active = Self::child_directory_identity(&registry_parent.file, &pinned.name);
            let archived = Self::child_directory_identity(&archive_parent.file, destination_name);
            let opened = pinned
                .file
                .metadata()
                .map_err(|error| fail(format!("cannot reinspect pinned agent directory: {error}")))
                .and_then(|metadata| {
                    Self::private_directory_identity(&metadata, "pinned agent directory")
                });
            match (active, archived, opened) {
                (Ok(Some(active)), _archived, Ok(opened))
                    if active == (pinned.device, pinned.inode)
                        && opened == (pinned.device, pinned.inode) =>
                {
                    return Err(rename_error);
                }
                (Ok(None), Ok(Some(archived)), Ok(opened))
                    if archived == (pinned.device, pinned.inode)
                        && opened == (pinned.device, pinned.inode) =>
                {
                    *published = true;
                    return match self.record_bytes_with(pinned, false) {
                        Ok(record) if record == expected_record => Err(fail(format!(
                            "archive rename reported failure ({rename_error}) after the proved generation was published"
                        ))),
                        Ok(_) => Err(fail(format!(
                            "archive rename reported failure ({rename_error}); the generation was published but its record changed"
                        ))),
                        Err(proof) => Err(fail(format!(
                            "archive rename reported failure ({rename_error}); the generation was published but its record could not be proved: {proof}"
                        ))),
                    };
                }
                (active, archived, opened) => {
                    *published = true;
                    return Err(fail(format!(
                        "archive rename reported failure ({rename_error}) with an ambiguous namespace outcome: active={active:?}, archive={archived:?}, opened={opened:?}"
                    )));
                }
            }
        }
        *published = true;
        after_rename();
        let result = (|| -> Result<()> {
            Self::verify_pinned_parent_directory(&registry_parent, "agent registry")?;
            Self::verify_pinned_parent_directory(&archive_parent, "agent archive")?;
            if Self::child_directory_identity(&registry_parent.file, &pinned.name)?.is_some() {
                return Err(fail(format!(
                    "agent {:?} active name reappeared during publication",
                    pinned.name
                )));
            }
            let published = Self::child_directory_identity(&archive_parent.file, destination_name)?
                .ok_or_else(|| fail("published agent directory disappeared"))?;
            let opened = pinned.file.metadata().map_err(|error| {
                fail(format!("cannot reinspect pinned agent directory: {error}"))
            })?;
            let opened_identity =
                Self::private_directory_identity(&opened, "pinned agent directory")?;
            if published != (pinned.device, pinned.inode)
                || opened_identity != (pinned.device, pinned.inode)
            {
                return Err(fail(format!(
                    "agent {:?} published directory was not the proved generation",
                    pinned.name
                )));
            }
            if self.record_bytes_with(pinned, false)? != expected_record {
                return Err(fail(format!(
                    "agent {:?} record changed during archival publication",
                    pinned.name
                )));
            }
            Ok(())
        })();
        if let Err(error) = result {
            // All agentctl participants hold the name lock. Reprove both
            // entries immediately before the inverse rename so a replacement
            // at either name is never promoted to active state. renameat2
            // cannot compare an inode atomically, so an actor that ignores the
            // cooperative lock can still race this last check; RENAME_NOREPLACE
            // at least refuses an occupied active name.
            let active = match Self::child_directory_identity(
                &registry_parent.file,
                &pinned.name,
            ) {
                Ok(value) => value,
                Err(proof) => {
                    return Err(fail(format!(
                        "archive identity check failed ({error}); cannot prove the active name absent before rollback: {proof}"
                    )))
                }
            };
            if active.is_some() {
                return Err(fail(format!(
                    "archive identity check failed ({error}); active name reappeared, so rollback was not attempted"
                )));
            }
            let rollback_source = match Self::child_directory_identity(
                &archive_parent.file,
                destination_name,
            ) {
                Ok(value) => value,
                Err(proof) => {
                    return Err(fail(format!(
                        "archive identity check failed ({error}); cannot reprove the published generation before rollback: {proof}"
                    )))
                }
            };
            if rollback_source != Some((pinned.device, pinned.inode)) {
                return Err(fail(format!(
                    "archive identity check failed ({error}); archive destination was replaced, so rollback was not attempted"
                )));
            }
            if let Err(rollback) = rename_directory_noreplace_at(
                &archive_parent.file,
                destination_name,
                &registry_parent.file,
                &pinned.name,
            ) {
                return Err(fail(format!(
                    "archive identity check failed ({error}) and rollback was incomplete: {rollback}"
                )));
            }
            after_rollback();
            let rollback_proof = (|| -> Result<()> {
                Self::verify_pinned_parent_directory(&registry_parent, "agent registry")?;
                Self::verify_pinned_parent_directory(&archive_parent, "agent archive")?;
                Self::verify_pinned_agent_directory(pinned)?;
                if Self::child_directory_identity(&registry_parent.file, &pinned.name)?
                    != Some((pinned.device, pinned.inode))
                {
                    return Err(fail("restored active name is not the proved generation"));
                }
                if Self::child_directory_identity(&archive_parent.file, destination_name)?.is_some()
                {
                    return Err(fail("archive destination remained after rollback"));
                }
                if self.record_bytes(pinned)? != expected_record {
                    return Err(fail("restored agent record is not the proved content"));
                }
                Ok(())
            })();
            if let Err(rollback_proof) = rollback_proof {
                return Err(fail(format!(
                    "archive identity check failed ({error}); inverse rename completed but rollback state could not be proved: {rollback_proof}"
                )));
            }
            let mut failures = Vec::new();
            for (directory, label) in [
                (&archive_parent.file, "agent archive rollback"),
                (&registry_parent.file, "agent registry rollback"),
            ] {
                if let Err(sync_error) = sync(directory, label) {
                    failures.push(format!("{label}: {sync_error}"));
                }
            }
            if !failures.is_empty() {
                return Err(fail(format!(
                    "archive identity check failed ({error}); rollback completed but directory durability is uncertain: {}",
                    failures.join("; ")
                )));
            }
            *published = false;
            return Err(error);
        }
        let mut failures = Vec::new();
        for (directory, label) in [
            (&archive_parent.file, "published agent archive"),
            (&registry_parent.file, "published agent registry"),
        ] {
            if let Err(sync_error) = sync(directory, label) {
                failures.push(format!("{label}: {sync_error}"));
            }
        }
        if !failures.is_empty() {
            return Err(fail(format!(
                "agent archive was published but directory durability is uncertain: {}",
                failures.join("; ")
            )));
        }
        Ok(())
    }

    fn unlink_pinned_file(pinned: &PinnedAgentDirectory, name: &str) -> Result<()> {
        let name = CString::new(name).map_err(|_| fail("registry artifact name contains NUL"))?;
        let result = unsafe { libc::unlinkat(pinned.file.as_raw_fd(), name.as_ptr(), 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(fail(format!("cannot remove recovery snapshot: {error}")));
            }
        }
        pinned
            .file
            .sync_all()
            .map_err(|error| fail(format!("cannot sync restored agent directory: {error}")))
    }

    fn restore_output_snapshot(
        pinned: &PinnedAgentDirectory,
        previous: Option<&[u8]>,
    ) -> Result<()> {
        match previous {
            Some(content) => atomic_replace_bytes(pinned, "output.json", content)
                .map(|_| ())
                .map_err(|error| *error.error),
            None => Self::unlink_pinned_file(pinned, "output.json"),
        }
    }

    fn verify_installed_output_snapshot(
        pinned: &PinnedAgentDirectory,
        installed: InstalledArtifact,
    ) -> Result<()> {
        let mut current = Self::open_pinned_file(pinned, "output.json", libc::O_RDONLY)?;
        let before = current
            .metadata()
            .map_err(|error| fail(format!("cannot reprove installed output: {error}")))?;
        let uid = unsafe { libc::getuid() };
        let mut content = Vec::with_capacity(installed.size as usize);
        Read::by_ref(&mut current)
            .take(installed.size + 1)
            .read_to_end(&mut content)
            .map_err(|error| fail(format!("cannot reread installed output: {error}")))?;
        let after = current
            .metadata()
            .map_err(|error| fail(format!("cannot reinspect installed output: {error}")))?;
        if !before.is_file()
            || before.uid() != uid
            || before.permissions().mode() & 0o077 != 0
            || before.nlink() != 1
            || before.len() != installed.size
            || content.len() as u64 != installed.size
            || <[u8; 32]>::from(Sha256::digest(&content)) != installed.digest
            || (before.dev(), before.ino()) != (installed.device, installed.inode)
            || after.dev() != before.dev()
            || after.ino() != before.ino()
            || after.mode() != before.mode()
            || after.uid() != before.uid()
            || after.nlink() != before.nlink()
            || after.len() != before.len()
            || after.mtime() != before.mtime()
            || after.mtime_nsec() != before.mtime_nsec()
            || after.ctime() != before.ctime()
            || after.ctime_nsec() != before.ctime_nsec()
        {
            return Err(fail(
                "installed output generation changed before use; replacement was preserved",
            ));
        }
        Ok(())
    }

    fn restore_installed_output_snapshot(
        pinned: &PinnedAgentDirectory,
        previous: Option<&[u8]>,
        installed: InstalledArtifact,
    ) -> Result<()> {
        Self::verify_installed_output_snapshot(pinned, installed)?;
        // Registry participants hold the per-name lock; a same-uid process
        // ignoring it can still race this final proof and replacement.
        Self::restore_output_snapshot(pinned, previous)
    }

    fn optional_snapshot_bytes(pinned: &PinnedAgentDirectory) -> Result<Option<Vec<u8>>> {
        let path = pinned.path.join("output.json");
        let Some(mut file) =
            Self::open_optional_pinned_file(pinned, "output.json", libc::O_RDONLY)?
        else {
            return Ok(None);
        };
        let before = file
            .metadata()
            .map_err(|error| fail(format!("cannot inspect output snapshot: {error}")))?;
        let uid = unsafe { libc::getuid() };
        if !before.is_file()
            || before.uid() != uid
            || before.permissions().mode() & 0o077 != 0
            || before.nlink() != 1
            || before.len() > MAX_SNAPSHOT_BYTES as u64
        {
            return Err(fail(format!(
                "unsafe existing output snapshot: {}",
                path.display()
            )));
        }
        let mut content = Vec::with_capacity(before.len() as usize);
        Read::by_ref(&mut file)
            .take((MAX_SNAPSHOT_BYTES + 1) as u64)
            .read_to_end(&mut content)
            .map_err(|error| fail(format!("cannot read output snapshot: {error}")))?;
        let after = file
            .metadata()
            .map_err(|error| fail(format!("cannot reinspect output snapshot: {error}")))?;
        if content.len() > MAX_SNAPSHOT_BYTES
            || before.len() != content.len() as u64
            || after.dev() != before.dev()
            || after.ino() != before.ino()
            || after.mode() != before.mode()
            || after.uid() != before.uid()
            || after.nlink() != before.nlink()
            || after.len() != before.len()
            || after.mtime() != before.mtime()
            || after.mtime_nsec() != before.mtime_nsec()
            || after.ctime() != before.ctime()
            || after.ctime_nsec() != before.ctime_nsec()
        {
            return Err(fail(format!(
                "existing output snapshot changed while reading: {}",
                path.display()
            )));
        }
        Ok(Some(content))
    }

    fn publish_archive(
        &self,
        pinned: &PinnedAgentDirectory,
        destination: &Path,
        snapshot: &Value,
        expected_record: &[u8],
        before_publish: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        self.publish_archive_with(
            pinned,
            destination,
            snapshot,
            expected_record,
            before_publish,
            (
                |_pinned, _temporary| Ok(()),
                |_pinned, _name, _temporary| Ok(()),
                rename_directory_noreplace_at,
                || {},
                || {},
                |directory: &File, _label| directory.sync_all(),
            ),
        )
    }

    fn publish_archive_with<AfterWrite, AfterInstall, Rename, AfterRename, AfterRollback, Sync>(
        &self,
        pinned: &PinnedAgentDirectory,
        destination: &Path,
        snapshot: &Value,
        expected_record: &[u8],
        before_publish: impl FnOnce() -> Result<()>,
        publication_hooks: (
            AfterWrite,
            AfterInstall,
            Rename,
            AfterRename,
            AfterRollback,
            Sync,
        ),
    ) -> Result<()>
    where
        AfterWrite: FnOnce(&PinnedAgentDirectory, &str) -> Result<()>,
        AfterInstall: FnOnce(&PinnedAgentDirectory, &str, &str) -> Result<()>,
        Rename: FnOnce(&File, &str, &File, &str) -> Result<()>,
        AfterRename: FnOnce(),
        AfterRollback: FnOnce(),
        Sync: FnMut(&File, &str) -> io::Result<()>,
    {
        let (after_write, after_install, rename, after_rename, after_rollback, sync) =
            publication_hooks;
        let previous = Self::optional_snapshot_bytes(pinned)?;
        let encoded = serde_json::to_vec_pretty(snapshot)
            .map_err(|error| fail(format!("cannot serialize output snapshot: {error}")))?;
        let mut encoded = encoded;
        encoded.push(b'\n');
        let installed = match atomic_replace_bytes_with(
            pinned,
            "output.json",
            &encoded,
            (after_write, after_install),
        ) {
            Ok(installed) => installed,
            Err(error) => match error.state {
                ArtifactInstallState::NotInstalled => return Err(*error.error),
                ArtifactInstallState::Uncertain => {
                    return Err(fail(format!(
                        "archive output installation is uncertain; replacement was preserved: {}",
                        error.error
                    )))
                }
                ArtifactInstallState::Installed(installed) => {
                    if let Err(rollback) = Self::restore_installed_output_snapshot(
                        pinned,
                        previous.as_deref(),
                        installed,
                    ) {
                        return Err(fail(format!(
                            "archive preparation failed ({}) and exact output rollback was unsafe or incomplete: {rollback}",
                            error.error
                        )));
                    }
                    return Err(*error.error);
                }
            },
        };
        if let Err(error) = Self::verify_installed_output_snapshot(pinned, installed) {
            return Err(fail(format!(
                "archive output changed after preparation; replacement was preserved: {error}"
            )));
        }
        if let Err(error) = before_publish() {
            if let Err(rollback) =
                Self::restore_installed_output_snapshot(pinned, previous.as_deref(), installed)
            {
                return Err(fail(format!(
                    "archive preparation failed ({error}) and exact output rollback was unsafe or incomplete: {rollback}"
                )));
            }
            return Err(error);
        }
        let mut published = false;
        if let Err(error) = self.publish_pinned_directory_with(
            pinned,
            destination,
            expected_record,
            &mut published,
            (rename, after_rename, after_rollback, sync),
        ) {
            if published {
                return Err(error);
            }
            let rollback =
                Self::restore_installed_output_snapshot(pinned, previous.as_deref(), installed);
            if let Err(rollback) = rollback {
                return Err(fail(format!(
                    "archive publication failed ({error}) and output rollback was incomplete: {rollback}"
                )));
            }
            return Err(error);
        }
        Ok(())
    }

    fn recover_legacy_adoption_locked(
        &self,
        record: &AgentRecord,
        pinned: &PinnedAgentDirectory,
        options: &StopOptions,
    ) -> Result<Value> {
        self.recover_legacy_adoption_locked_with(record, pinned, options, || {})
    }

    fn recover_legacy_adoption_locked_with(
        &self,
        record: &AgentRecord,
        pinned: &PinnedAgentDirectory,
        options: &StopOptions,
        after_output_preparation: impl FnOnce(),
    ) -> Result<Value> {
        let expected_token = options.expected_token.as_deref().ok_or_else(|| {
            fail("legacy adoption recovery requires --expected-token and --expected-record-sha256")
        })?;
        let expected_hash = options.expected_record_sha256.as_deref().ok_or_else(|| {
            fail("legacy adoption recovery requires --expected-token and --expected-record-sha256")
        })?;
        let initial = self.legacy_record_snapshot(pinned, expected_token, expected_hash)?;
        if &initial.record != record {
            return Err(fail(format!(
                "refusing to recover adoption {:?}: registry record changed",
                record.name
            )));
        }
        let record = &initial.record;
        if record.launch.adapter != "herdr-foreign"
            || record.lifecycle != "running"
            || record.launch.mode != "interactive"
            || record.launch.backend != "herdr"
            || record.foreign_shell_identity.is_some()
        {
            return Err(fail(
                "--recover-legacy-adoption applies only to a running herdr-foreign record missing foreign_shell_identity",
            ));
        }
        let before = self.dead_pane_proof(record, "recover legacy adoption")?;
        let text = self.bounded_terminal_text(&before.info.pane_id)?;
        let current_snapshot =
            self.legacy_record_snapshot(pinned, expected_token, expected_hash)?;
        if !current_snapshot.same_generation(&initial) {
            return Err(fail(format!(
                "refusing to recover legacy adoption {:?}: registry record changed",
                record.name
            )));
        }
        let current = &current_snapshot.record;
        let after = self.dead_pane_proof(current, "recover legacy adoption")?;
        if after != before {
            return Err(fail(format!(
                "refusing to recover legacy adoption {:?}: runtime identity changed during output capture",
                record.name
            )));
        }
        let (_archive, destination) = self.archive_destination(current)?;
        let captured = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_secs_f64();
        self.publish_archive(
            pinned,
            &destination,
            &json!({
                "text": text,
                "captured_at": captured,
                "pane_id": current.pane_id,
                "recovery_shell_identity": Self::shell_proof_json(&before.shell),
            }),
            &initial.content,
            || {
                after_output_preparation();
                let final_snapshot =
                    self.legacy_record_snapshot(pinned, expected_token, expected_hash)?;
                if !final_snapshot.same_generation(&initial) {
                    return Err(fail(format!(
                        "refusing to recover legacy adoption {:?}: registry record changed",
                        record.name
                    )));
                }
                if self.dead_pane_proof(&final_snapshot.record, "recover legacy adoption")?
                    != before
                {
                    return Err(fail(format!(
                        "refusing to recover legacy adoption {:?}: runtime identity changed before archival",
                        record.name
                    )));
                }
                Ok(())
            },
        )?;
        Ok(json!({
            "name": record.name,
            "archive": destination,
            "pane_closed": false,
            "tab_closed": false,
            "runtime_preserved": true,
            "recovered_legacy_adoption": true,
            "record_sha256": expected_hash,
            "recovery_shell_identity": Self::shell_proof_json(&before.shell),
        }))
    }

    fn retire_managed_dead_locked(
        &self,
        record: &AgentRecord,
        pinned: &PinnedAgentDirectory,
        expected_token: Option<&str>,
    ) -> Result<Value> {
        self.retire_managed_dead_locked_with(
            record,
            pinned,
            expected_token,
            || {},
            || {},
            (
                |_pinned, _temporary| Ok(()),
                |_pinned, _name, _temporary| Ok(()),
            ),
        )
    }

    fn managed_dead_retirement_value(record: &AgentRecord, record_bytes: &[u8]) -> Value {
        json!({
            "schema": MANAGED_DEAD_RETIREMENT_SCHEMA,
            "name": record.name,
            "token": record.token,
            "record_sha256": format!("{:x}", Sha256::digest(record_bytes)),
            "pane_closed": false,
            "runtime_preserved": true,
        })
    }

    fn managed_dead_retirement_exists(&self, record: &AgentRecord) -> Result<bool> {
        let pinned = self.pinned_agent_directory(&record.name)?;
        let Some(file) =
            Self::open_optional_pinned_file(&pinned, MANAGED_DEAD_RETIREMENT_FILE, libc::O_RDONLY)?
        else {
            Self::verify_pinned_agent_directory(&pinned)?;
            return Ok(false);
        };
        let metadata = file.metadata().map_err(|error| {
            fail(format!(
                "cannot inspect managed-dead retirement receipt: {error}"
            ))
        })?;
        let uid = unsafe { libc::getuid() };
        if !metadata.is_file()
            || metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(fail("unsafe managed-dead retirement receipt"));
        }
        Self::verify_pinned_agent_directory(&pinned)?;
        Ok(true)
    }

    fn read_managed_dead_retirement(
        &self,
        pinned: &PinnedAgentDirectory,
        record: &AgentRecord,
        record_bytes: &[u8],
    ) -> Result<Value> {
        let bytes = Self::pinned_artifact_bytes(
            pinned,
            MANAGED_DEAD_RETIREMENT_FILE,
            MAX_AGENT_RECORD_BYTES,
            "managed-dead retirement receipt",
            true,
        )?;
        let document = agent::decode_json_strict(&bytes).map_err(|error| {
            fail(format!(
                "cannot decode managed-dead retirement receipt: {error}"
            ))
        })?;
        let expected = Self::managed_dead_retirement_value(record, record_bytes);
        if document != expected {
            return Err(fail(
                "managed-dead retirement receipt disagrees with its stopped record",
            ));
        }
        Ok(expected)
    }

    fn managed_dead_result(name: &str, destination: &Path) -> Value {
        json!({
            "name": name,
            "archive": destination,
            "pane_closed": false,
            "tab_closed": false,
            "managed_dead": true,
            "runtime_preserved": true,
            "continuation": "The dead custom runtime was archived; its shell pane was preserved because Herdr does not provide a terminal-generation-conditional close.",
        })
    }

    fn managed_dead_archive_receipt(
        &self,
        agent_name: &str,
        expected_token: Option<&str>,
    ) -> Result<Option<Value>> {
        self.managed_dead_archive_receipt_with(agent_name, expected_token, || {}, || {})
    }

    fn managed_dead_archive_receipt_with<AfterRecord, AfterReceipt>(
        &self,
        agent_name: &str,
        expected_token: Option<&str>,
        after_record_read: AfterRecord,
        after_receipt_read: AfterReceipt,
    ) -> Result<Option<Value>>
    where
        AfterRecord: FnOnce(),
        AfterReceipt: FnOnce(),
    {
        let Some(expected_token) = expected_token else {
            return Ok(None);
        };
        match fs::symlink_metadata(self.directory(agent_name)?) {
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(fail(format!(
                    "cannot inspect active managed-dead record: {error}"
                )))
            }
        }
        let destination = self
            .registry
            .join("archive")
            .join(format!("{agent_name}-{expected_token}"));
        match fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(fail(format!(
                    "cannot inspect managed-dead archive: {error}"
                )))
            }
            Ok(_) => {}
        }
        let pinned = Self::pinned_agent_directory_at(
            agent_name,
            destination.clone(),
            "managed-dead agent archive",
        )?;
        let record_path = destination.join("agent.json");
        let record_bytes = Self::pinned_artifact_bytes(
            &pinned,
            "agent.json",
            MAX_AGENT_RECORD_BYTES,
            "managed-dead archived record",
            true,
        )?;
        let record = AgentRecord::from_storage_value(
            agent::decode_json_strict(&record_bytes).map_err(|error| {
                fail(format!(
                    "cannot decode managed-dead archived record: {error}"
                ))
            })?,
            &record_path,
            agent_name,
        )?;
        if record.token != expected_token
            || record.lifecycle != "stopped"
            || record.launch.adapter != "herdr-pane"
            || record.launch.harness != "muse"
            || record.launch.mode != "interactive"
            || record.launch.backend != "herdr"
            || record.launch.runtime_ownership != "owned"
            || record.custom_process_identity.is_none()
            || record.workspace_id.is_none()
            || record.tab_id.is_none()
            || record.pane_id.is_none()
        {
            return Err(fail(
                "managed-dead archive is not an exact preserved-runtime receipt",
            ));
        }
        after_record_read();
        self.read_managed_dead_retirement(&pinned, &record, &record_bytes)?;
        after_receipt_read();
        Self::verify_pinned_agent_directory(&pinned)?;
        Ok(Some(Self::managed_dead_result(agent_name, &destination)))
    }

    fn complete_preserved_managed_dead_publication(
        &self,
        pinned: &PinnedAgentDirectory,
        expected_token: &str,
    ) -> Result<Value> {
        let snapshot = self.managed_record_snapshot(pinned, expected_token)?;
        let final_record = snapshot.record;
        if final_record.lifecycle != "stopped"
            || final_record.launch.adapter != "herdr-pane"
            || final_record.launch.mode != "interactive"
            || final_record.launch.backend != "herdr"
        {
            return Err(fail(
                "preserved managed-dead publication requires an exact stopped herdr-pane record",
            ));
        }
        self.read_managed_dead_retirement(pinned, &final_record, &snapshot.content)?;
        let (_archive, destination) = self.archive_destination(&final_record)?;
        self.publish_pinned_directory(pinned, &destination, &snapshot.content)?;
        Ok(self
            .managed_dead_archive_receipt(&final_record.name, Some(expected_token))?
            .unwrap_or_else(|| Self::managed_dead_result(&final_record.name, &destination)))
    }

    fn complete_ordinary_stopped_custom_publication(
        &self,
        record: &AgentRecord,
        expected_token: Option<&str>,
    ) -> Result<Value> {
        let expected_token = expected_token.ok_or_else(|| {
            fail(format!(
                "recovering stopped custom agent {:?} requires --expected-token",
                record.name
            ))
        })?;
        let pinned = self.pinned_agent_directory(&record.name)?;
        let snapshot = self.managed_record_snapshot(&pinned, expected_token)?;
        let current = snapshot.record;
        if current.lifecycle != "stopped"
            || current.launch.adapter != "herdr-pane"
            || current.launch.harness != "muse"
            || current.launch.mode != "interactive"
            || current.launch.backend != "herdr"
            || current.launch.runtime_ownership != "owned"
            || current.pane_id.is_none()
        {
            return Err(fail(
                "ordinary stopped custom publication has inconsistent state",
            ));
        }
        let Some(pane_id) = current.pane_id.as_deref() else {
            return Err(fail(
                "ordinary stopped custom publication has no pane identity",
            ));
        };
        if self
            .client
            .panes()?
            .iter()
            .any(|pane| pane.pane_id == pane_id)
        {
            return Err(fail(
                "stopped custom runtime still has a pane but no managed-dead retirement receipt; pane was preserved",
            ));
        }
        let (_archive, destination) = self.archive_destination(&current)?;
        self.publish_pinned_directory(&pinned, &destination, &snapshot.content)?;
        Ok(json!({
            "name": current.name,
            "archive": destination,
            "pane_closed": false,
            "tab_closed": Value::Null,
            "ordinary_stop_recovered": true,
            "runtime_preserved": false,
        }))
    }

    fn retire_managed_dead_locked_with<AfterWrite, AfterInstall>(
        &self,
        record: &AgentRecord,
        pinned: &PinnedAgentDirectory,
        expected_token: Option<&str>,
        after_preparation: impl FnOnce(),
        after_final_record_proof: impl FnOnce(),
        artifact_hooks: (AfterWrite, AfterInstall),
    ) -> Result<Value>
    where
        AfterWrite: FnOnce(&PinnedAgentDirectory, &str) -> Result<()>,
        AfterInstall: FnOnce(&PinnedAgentDirectory, &str, &str) -> Result<()>,
    {
        let expected_token = expected_token.ok_or_else(|| {
            fail(format!(
                "retiring dead managed agent {:?} requires --expected-token",
                record.name
            ))
        })?;
        let initial = self.managed_record_snapshot(pinned, expected_token)?;
        if &initial.record != record {
            return Err(fail(format!(
                "refusing to retire dead managed agent {:?}: registry record changed",
                record.name
            )));
        }
        let record = &initial.record;
        if record.lifecycle == "stopped" && record.launch.adapter == "herdr-pane" {
            return self.complete_preserved_managed_dead_publication(pinned, expected_token);
        }
        if !matches!(record.launch.adapter.as_str(), "herdr" | "herdr-pane")
            || record.lifecycle != "running"
            || record.launch.mode != "interactive"
            || record.launch.backend != "herdr"
        {
            return Err(fail(
                "managed-dead retirement requires a running herdr record with owned interactive routing",
            ));
        }
        let before = self.dead_pane_proof(record, "retire dead managed agent")?;
        let text = self.bounded_terminal_text(&before.info.pane_id)?;
        let current_snapshot = self.managed_record_snapshot(pinned, expected_token)?;
        if !current_snapshot.same_generation(&initial) {
            return Err(fail(format!(
                "refusing to retire dead managed agent {:?}: registry record changed",
                record.name
            )));
        }
        let current = &current_snapshot.record;
        if self.dead_pane_proof(current, "retire dead managed agent")? != before {
            return Err(fail(format!(
                "refusing to retire dead managed agent {:?}: runtime identity changed during output capture",
                record.name
            )));
        }
        let (_archive, destination) = self.archive_destination(current)?;
        let final_snapshot = self.managed_record_snapshot(pinned, expected_token)?;
        if !final_snapshot.same_generation(&initial) {
            return Err(fail(format!(
                "refusing to retire dead managed agent {:?}: registry record changed",
                record.name
            )));
        }
        if self.dead_pane_proof(&final_snapshot.record, "retire dead managed agent")? != before {
            return Err(fail(format!(
                "refusing to retire dead managed agent {:?}: runtime identity changed before close",
                record.name
            )));
        }
        let mut final_record = final_snapshot.record;
        final_record.lifecycle = "stopped".to_owned();
        self.migrate_legacy_goal_messages(&final_record)?;
        let mut stopped_bytes = serde_json::to_vec_pretty(&final_record.storage_value()?)
            .map_err(|error| fail(format!("cannot serialize agent record: {error}")))?;
        stopped_bytes.push(b'\n');
        if stopped_bytes.len() > MAX_AGENT_RECORD_BYTES {
            return Err(fail(format!(
                "refusing stopped agent record larger than {MAX_AGENT_RECORD_BYTES} bytes"
            )));
        }
        let captured = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_secs_f64();
        let previous_output = Self::optional_snapshot_bytes(pinned)?;
        let snapshot = json!({
            "text": text,
            "captured_at": captured,
            "pane_id": current.pane_id,
            "retirement_shell_identity": Self::shell_proof_json(&before.shell),
        });
        let mut snapshot_bytes = serde_json::to_vec_pretty(&snapshot)
            .map_err(|error| fail(format!("cannot serialize output snapshot: {error}")))?;
        snapshot_bytes.push(b'\n');
        let installed = match atomic_replace_bytes_with(
            pinned,
            "output.json",
            &snapshot_bytes,
            artifact_hooks,
        ) {
            Ok(installed) => installed,
            Err(error) => match error.state {
                ArtifactInstallState::NotInstalled => return Err(*error.error),
                ArtifactInstallState::Uncertain => {
                    return Err(fail(format!(
                        "managed retirement output installation is uncertain; replacement was preserved: {}",
                        error.error
                    )))
                }
                ArtifactInstallState::Installed(installed) => {
                    if let Err(rollback) = Self::restore_installed_output_snapshot(
                        pinned,
                        previous_output.as_deref(),
                        installed,
                    ) {
                        return Err(fail(format!(
                            "managed retirement preparation failed ({}) and exact output rollback was unsafe or incomplete: {rollback}",
                            error.error
                        )));
                    }
                    return Err(*error.error);
                }
            },
        };
        after_preparation();
        let final_runtime_proof = (|| -> Result<()> {
            Self::verify_installed_output_snapshot(pinned, installed)?;
            if self.record_bytes(pinned)? != initial.content {
                return Err(fail(format!(
                    "refusing to retire dead managed agent {:?}: registry record changed immediately before close",
                    record.name
                )));
            }
            after_final_record_proof();
            self.dead_pane_proof(&final_record, "retire dead managed agent")
                .and_then(|proof| {
                if proof == before {
                    Ok(())
                } else {
                    Err(fail(format!(
                        "refusing to retire dead managed agent {:?}: runtime identity changed immediately before close",
                        record.name
                    )))
                }
            })?;
            Ok(())
        })();
        if let Err(error) = final_runtime_proof {
            let rollback = Self::restore_installed_output_snapshot(
                pinned,
                previous_output.as_deref(),
                installed,
            );
            if let Err(rollback) = rollback {
                return Err(fail(format!(
                    "managed retirement final runtime proof failed ({error}) and rollback was incomplete: {rollback}"
                )));
            }
            return Err(error);
        }
        let pane_id = final_record
            .pane_id
            .as_deref()
            .expect("proved pane identity");
        let close_pane = final_record.launch.adapter == "herdr";
        if close_pane {
            self.client.close_pane(pane_id)?;
        } else {
            let receipt = Self::managed_dead_retirement_value(&final_record, &stopped_bytes);
            let mut receipt_bytes = serde_json::to_vec_pretty(&receipt)
                .map_err(|error| fail(format!("cannot serialize retirement receipt: {error}")))?;
            receipt_bytes.push(b'\n');
            atomic_replace_bytes(pinned, MANAGED_DEAD_RETIREMENT_FILE, &receipt_bytes)
                .map_err(|error| *error.error)?;
        }
        atomic_replace_bytes(pinned, "agent.json", &stopped_bytes).map_err(|error| *error.error)?;
        self.publish_pinned_directory(pinned, &destination, &stopped_bytes)?;
        let tab_closed = if close_pane {
            self.client.panes().ok().map(|panes| {
                panes
                    .iter()
                    .all(|pane| Some(&pane.tab_id) != final_record.tab_id.as_ref())
            })
        } else {
            Some(false)
        };
        if close_pane {
            Ok(json!({
                "name": record.name,
                "archive": destination,
                "pane_closed": true,
                "tab_closed": tab_closed,
                "managed_dead": true,
                "runtime_preserved": false,
                "continuation": Value::Null,
            }))
        } else {
            Ok(Self::managed_dead_result(&record.name, &destination))
        }
    }

    /// Close an owned pane, or only unregister a foreign runtime, then archive state.
    pub fn stop(&self, agent_name: &str) -> Result<Value> {
        self.stop_with_options(agent_name, StopOptions::default())
    }

    /// Stop with explicit generation assertions or the loud adoption-recovery gate.
    pub fn stop_with_options(&self, agent_name: &str, options: StopOptions) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        if !options.recover_legacy_adoption && options.expected_record_sha256.is_none() {
            if let Some(receipt) =
                self.managed_dead_archive_receipt(agent_name, options.expected_token.as_deref())?
            {
                return Ok(receipt);
            }
        }
        let mut record = self.load(agent_name)?;
        self.reconcile_goal_transaction(&record)?;
        record.supported()?;
        if options
            .expected_token
            .as_deref()
            .is_some_and(|expected| expected != record.token)
        {
            return Err(fail(format!(
                "agent {agent_name:?} was replaced before this operation"
            )));
        }
        let confirmed_record = self.load(agent_name)?;
        if confirmed_record.token != record.token
            || confirmed_record.public_value() != record.public_value()
        {
            return Err(fail(format!(
                "agent {agent_name:?} record changed before stop"
            )));
        }
        record = confirmed_record;
        if options.recover_legacy_adoption {
            let pane_id = record
                .pane_id
                .as_deref()
                .ok_or_else(|| fail("legacy adopted record has no pane identity"))?;
            let _pane_lock = self.pane_lock(pane_id)?;
            let pinned = self.pinned_agent_directory(agent_name)?;
            return self.recover_legacy_adoption_locked(&record, &pinned, &options);
        }
        if options.expected_record_sha256.is_some() {
            return Err(fail(
                "--expected-record-sha256 requires --recover-legacy-adoption",
            ));
        }
        if record.launch.adapter == "herdr-pane" && record.lifecycle == "stopped" {
            if self.managed_dead_retirement_exists(&record)? {
                let pane_id = record
                    .pane_id
                    .as_deref()
                    .ok_or_else(|| fail("managed-dead stopped record has no pane identity"))?;
                let _pane_lock = self.pane_lock(pane_id)?;
                let pinned = self.pinned_agent_directory(agent_name)?;
                return self.retire_managed_dead_locked(
                    &record,
                    &pinned,
                    options.expected_token.as_deref(),
                );
            }
            return self.complete_ordinary_stopped_custom_publication(
                &record,
                options.expected_token.as_deref(),
            );
        }
        if record.launch.adapter == "herdr-foreign" {
            let (live, info, presentation) = self.inspect_foreign(agent_name, &record)?;
            let text = self.bounded_terminal_text(&info.pane_id)?;
            // Output capture is another control round trip. Refuse archival if
            // the exact foreign identity changed while it was in progress.
            let (final_live, final_info, final_presentation) = self
                .inspect_foreign(agent_name, &record)
                .map_err(|error| {
                    fail(format!(
                        "refusing to unregister adopted agent {agent_name:?}: runtime identity could not be reverified after output capture: {error}"
                    ))
                })?;
            let cwd_stable = final_info.cwd == info.cwd
                || fs::canonicalize(&final_info.cwd)
                    .ok()
                    .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&info.cwd).ok());
            if final_live != live
                || final_info.pane_id != info.pane_id
                || final_info.workspace_id != info.workspace_id
                || !cwd_stable
                || final_info.agent != info.agent
                || final_info.session_agent != info.session_agent
                || final_info.session_value != info.session_value
                || final_presentation != presentation
            {
                return Err(fail(format!(
                    "refusing to unregister adopted agent {agent_name:?}: runtime identity changed during output capture"
                )));
            }
            let (archive, destination) = self.archive_destination(&record)?;
            self.snapshot(&record, &text)?;
            let (persisted_live, persisted_info, persisted_presentation) = self
                .inspect_foreign(agent_name, &record)
                .map_err(|error| {
                    fail(format!(
                        "refusing to unregister adopted agent {agent_name:?}: runtime identity could not be reverified before archival: {error}"
                    ))
                })?;
            let persisted_cwd_stable = persisted_info.cwd == info.cwd
                || fs::canonicalize(&persisted_info.cwd)
                    .ok()
                    .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&info.cwd).ok());
            if persisted_live != live
                || persisted_info.pane_id != info.pane_id
                || persisted_info.workspace_id != info.workspace_id
                || !persisted_cwd_stable
                || persisted_info.agent != info.agent
                || persisted_info.session_agent != info.session_agent
                || persisted_info.session_value != info.session_value
                || persisted_presentation != presentation
            {
                return Err(fail(format!(
                    "refusing to unregister adopted agent {agent_name:?}: runtime identity changed before archival"
                )));
            }
            record.lifecycle = "stopped".to_owned();
            self.save(&record)?;
            fs::rename(self.directory(agent_name)?, &destination)
                .map_err(|error| fail(error.to_string()))?;
            agent::sync_directory(&archive)?;
            agent::sync_directory(&self.registry)?;
            return Ok(json!({
                "name": agent_name,
                "archive": destination,
                "pane_closed": false,
                "tab_closed": false,
                "runtime_preserved": true,
            }));
        }
        let panes = self.client.panes()?;
        if matches!(record.launch.adapter.as_str(), "herdr" | "herdr-pane")
            && matches!(record.lifecycle.as_str(), "running" | "stopping")
            && record.pane_id.is_some()
        {
            let recorded: Vec<&Pane> = panes
                .iter()
                .filter(|pane| Some(&pane.pane_id) == record.pane_id.as_ref())
                .collect();
            if recorded.len() == 1 {
                let info = self.client.pane_info(&recorded[0].pane_id)?;
                let dead = if record.launch.adapter == "herdr" {
                    info.agent.is_none()
                } else if let Some(identity) = record.custom_process_identity.as_ref() {
                    self.client.process_generation_absent(identity)?
                } else {
                    false
                };
                if !dead {
                    // Continue through the ordinary live-session teardown path.
                } else {
                    if record.lifecycle != "running" {
                        return Err(fail(
                            "managed-dead retirement requires a running herdr record with owned interactive routing",
                        ));
                    }
                    let _pane_lock = self.pane_lock(&recorded[0].pane_id)?;
                    let pinned = self.pinned_agent_directory(agent_name)?;
                    return self.retire_managed_dead_locked(
                        &record,
                        &pinned,
                        options.expected_token.as_deref(),
                    );
                }
            }
        }
        if panes.iter().any(|pane| {
            Some(&pane.pane_id) == record.pane_id.as_ref()
                && Some(&pane.tab_id) != record.tab_id.as_ref()
        }) {
            return Err(fail(
                "refusing to archive an agent whose pane moved to another tab",
            ));
        }
        let owned: Vec<Pane> = panes
            .into_iter()
            .filter(|pane| Some(&pane.tab_id) == record.tab_id.as_ref())
            .collect();
        if record.pane_id.is_none() && record.lifecycle == "launch_failed" && owned.len() == 1 {
            let info = self.client.pane_info(&owned[0].pane_id)?;
            if info.agent.is_none()
                && Some(&info.workspace_id) == record.workspace_id.as_ref()
                && (info.cwd == record.launch.cwd
                    || fs::canonicalize(&info.cwd)
                        .ok()
                        .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.launch.cwd).ok()))
            {
                record.pane_id = Some(owned[0].pane_id.clone());
                self.save(&record)?;
            }
        }
        let (archive, destination) = self.archive_destination(&record)?;
        if !owned.is_empty() {
            if owned.len() != 1
                || Some(&owned[0].pane_id) != record.pane_id.as_ref()
                || Some(&owned[0].workspace_id) != record.workspace_id.as_ref()
            {
                return Err(fail("refusing to close a tab whose pane ownership changed"));
            }
            if record.launch.adapter == "herdr-pane"
                || record.lifecycle == "running"
                || self.client.pane_info(&owned[0].pane_id)?.agent.is_some()
            {
                self.checked_or_launch_failed(&record, &owned[0].pane_id)?;
            }
            let pane_id = &owned[0].pane_id;
            let mut text = self.client.read(pane_id, "recent-unwrapped", Some(5000))?;
            if text.is_empty() {
                text = self.client.read(pane_id, "recent", Some(5000))?;
            }
            self.snapshot(&record, &text)?;
            self.checked_or_launch_failed(&record, pane_id)?;
            record.lifecycle = "stopping".to_owned();
            self.save(&record)?;
            // A human can add a pane after the membership snapshot. Target the recorded
            // pane, never the whole tab; a newly added sibling must remain untouched.
            self.client.close_pane(pane_id)?;
        }
        record.lifecycle = "stopped".to_owned();
        self.save(&record)?;
        fs::rename(self.directory(agent_name)?, &destination)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&archive)?;
        agent::sync_directory(&self.registry)?;
        let tab_closed = if owned.is_empty() {
            Some(false)
        } else {
            self.client.panes().ok().map(|panes| {
                panes
                    .iter()
                    .all(|pane| Some(&pane.tab_id) != record.tab_id.as_ref())
            })
        };
        Ok(
            json!({"name":agent_name,"archive":destination,"pane_closed":!owned.is_empty(),"tab_closed":tab_closed}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AdapterError, Result as AdapterResult};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    #[derive(Default)]
    struct CancelLifecycleWait {
        cancelled: AtomicBool,
    }

    impl agent::AgentRuntime for CancelLifecycleWait {
        fn monotonic(&self) -> Duration {
            Duration::ZERO
        }

        fn sleep(&self, _duration: Duration) {
            self.cancelled.store(true, Ordering::SeqCst);
        }

        fn cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
    }
    struct Fixture {
        root: PathBuf,
        client: Fake,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentctl-managed-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                client: Fake {
                    root: root.clone(),
                    panes: Mutex::new(Vec::new()),
                    panes_calls: AtomicU64::new(0),
                    pane_info_calls: AtomicU64::new(0),
                    runs: Mutex::new(Vec::new()),
                    environments: Mutex::new(Vec::new()),
                    closed: Mutex::new(Vec::new()),
                    focused: Mutex::new(Vec::new()),
                    fail_panes: AtomicBool::new(false),
                    add_sibling_on_read: AtomicBool::new(false),
                    require_start_lock: AtomicBool::new(false),
                    started: AtomicBool::new(false),
                    report_session: AtomicBool::new(true),
                    duplicate_session: AtomicBool::new(false),
                    change_session_after_save: AtomicBool::new(false),
                    change_owned_session_after_save: AtomicBool::new(false),
                    fail_close: AtomicBool::new(false),
                    fail_after_close: AtomicBool::new(false),
                    custom_reported: AtomicBool::new(false),
                    custom_alive: AtomicBool::new(false),
                    custom_dies_after_report: AtomicBool::new(false),
                    custom_dies_during_recovery_commit: AtomicBool::new(false),
                    custom_fails_after_identity: AtomicBool::new(false),
                    fail_custom_verify_once: AtomicBool::new(false),
                    custom_verify_calls: AtomicU64::new(0),
                    custom_ready: AtomicBool::new(true),
                    custom_native_working: AtomicBool::new(false),
                    wait_fails: AtomicBool::new(false),
                    working_after_run: AtomicBool::new(false),
                    custom_screen: Mutex::new(None),
                    screen_after_run: Mutex::new(None),
                    screen_after_enter: Mutex::new(None),
                    ignore_first_enter: AtomicBool::new(false),
                    sent_texts: Mutex::new(Vec::new()),
                    sent_keys: Mutex::new(Vec::new()),
                    current_owned_pane: Mutex::new("owned".to_owned()),
                    move_count: AtomicU64::new(0),
                    replace_terminal_before_move: AtomicBool::new(false),
                    fail_after_move: AtomicBool::new(false),
                    wrong_move_response: AtomicBool::new(false),
                    ambiguous_workspace: AtomicBool::new(false),
                    custom_at_idle_shell: AtomicBool::new(true),
                    claude_background: AtomicBool::new(false),
                    foreign_shell_identity: Mutex::new(Fake::foreign_shell_identity()),
                    foreign_shell_path: Mutex::new(PathBuf::from("/bin/bash")),
                    replacement_shell_on_read: Mutex::new(None),
                    change_foreign_shell_after_save: AtomicBool::new(false),
                    change_foreign_shell_on_read: AtomicBool::new(false),
                    change_record_token_on_read: AtomicBool::new(false),
                    add_null_shell_identity_on_read: AtomicBool::new(false),
                    replace_record_directory_on_read: AtomicBool::new(false),
                    replace_after_empty_panes: AtomicBool::new(false),
                    fail_read: AtomicBool::new(false),
                    fail_shell_proof: AtomicBool::new(false),
                    shell_proof_mutation: AtomicU64::new(0),
                    oversized_read: AtomicBool::new(false),
                    non_ascii_read: AtomicBool::new(false),
                    restart_foreign_on_read: AtomicBool::new(false),
                    leave_idle_shell_on_read: AtomicBool::new(false),
                    wrong_foreign_cwd: AtomicBool::new(false),
                },
                root,
            }
        }
        fn manager(&self) -> ManagedAgents<'_, Fake> {
            ManagedAgents::new(&self.client, &self.root.join("registry"))
                .unwrap()
                .with_inherited_workspace(None)
        }
        fn start(&self, brief: Option<String>) -> Value {
            self.manager()
                .start(
                    "worker",
                    &self.root,
                    StartOptions {
                        workspace_id: Some("workspace".to_owned()),
                        brief,
                        ..StartOptions::default()
                    },
                )
                .unwrap()
        }
        fn prepare_foreign(&self) {
            self.client.panes.lock().unwrap().push(Fake::pane("owned"));
            self.client.started.store(true, Ordering::Relaxed);
        }
        fn adopt_options(&self) -> AdoptOptions {
            AdoptOptions {
                pane_id: "owned".to_owned(),
                expected_workspace: "subagents".to_owned(),
                cwd: self.root.clone(),
                harness: "codex".to_owned(),
                session: None,
            }
        }
        fn adopt(&self) -> Value {
            self.prepare_foreign();
            self.manager()
                .adopt("foreign", self.adopt_options())
                .unwrap()
        }

        fn make_legacy_dead(&self) -> (String, String, Vec<u8>) {
            let adopted = self.adopt();
            let path = self.root.join("registry/foreign/agent.json");
            let mut document = self.manager().load("foreign").unwrap().public_value();
            document
                .as_object_mut()
                .unwrap()
                .remove("foreign_shell_identity");
            agent::atomic_json(&path, &document).unwrap();
            let raw = fs::read(&path).unwrap();
            self.client.started.store(false, Ordering::Relaxed);
            self.client.report_session.store(false, Ordering::Relaxed);
            (
                adopted["token"].as_str().unwrap().to_owned(),
                format!("{:x}", Sha256::digest(&raw)),
                raw,
            )
        }

        fn make_managed_dead(&self) -> (String, String) {
            let started = self.start(None);
            self.client.started.store(false, Ordering::Relaxed);
            self.client.report_session.store(false, Ordering::Relaxed);
            (
                started["pane_id"].as_str().unwrap().to_owned(),
                started["token"].as_str().unwrap().to_owned(),
            )
        }
    }

    #[test]
    fn service_cancellation_bounds_lifecycle_lock_contention() {
        let fixture = Fixture::new();
        let registry = fixture.root.join("registry");
        let manager = ManagedAgents::new(&fixture.client, &registry).expect("manager");
        agent::create_private_directory(&registry, "agent registry", true, true).expect("registry");
        let path = registry.join(".coordinator.lock");
        let holder = agent::open_private_lock(&path, "held lifecycle lock").expect("holder");
        holder.lock_exclusive().expect("hold lifecycle lock");
        let runtime = CancelLifecycleWait::default();
        let started = Instant::now();
        let error = manager
            .lock_with_runtime("coordinator", &runtime)
            .expect_err("cancel contended lifecycle lock");
        assert!(error.to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    struct Fake {
        root: PathBuf,
        panes: Mutex<Vec<Pane>>,
        panes_calls: AtomicU64,
        pane_info_calls: AtomicU64,
        runs: Mutex<Vec<String>>,
        environments: Mutex<Vec<Vec<String>>>,
        closed: Mutex<Vec<String>>,
        focused: Mutex<Vec<String>>,
        fail_panes: AtomicBool,
        add_sibling_on_read: AtomicBool,
        require_start_lock: AtomicBool,
        started: AtomicBool,
        report_session: AtomicBool,
        duplicate_session: AtomicBool,
        change_session_after_save: AtomicBool,
        change_owned_session_after_save: AtomicBool,
        fail_close: AtomicBool,
        fail_after_close: AtomicBool,
        custom_reported: AtomicBool,
        custom_alive: AtomicBool,
        custom_dies_after_report: AtomicBool,
        custom_dies_during_recovery_commit: AtomicBool,
        custom_fails_after_identity: AtomicBool,
        fail_custom_verify_once: AtomicBool,
        custom_verify_calls: AtomicU64,
        custom_ready: AtomicBool,
        custom_native_working: AtomicBool,
        wait_fails: AtomicBool,
        working_after_run: AtomicBool,
        custom_screen: Mutex<Option<String>>,
        screen_after_run: Mutex<Option<String>>,
        screen_after_enter: Mutex<Option<String>>,
        ignore_first_enter: AtomicBool,
        sent_texts: Mutex<Vec<String>>,
        sent_keys: Mutex<Vec<String>>,
        current_owned_pane: Mutex<String>,
        move_count: AtomicU64,
        replace_terminal_before_move: AtomicBool,
        fail_after_move: AtomicBool,
        wrong_move_response: AtomicBool,
        ambiguous_workspace: AtomicBool,
        custom_at_idle_shell: AtomicBool,
        claude_background: AtomicBool,
        foreign_shell_identity: Mutex<CustomProcessIdentity>,
        foreign_shell_path: Mutex<PathBuf>,
        replacement_shell_on_read: Mutex<Option<PaneShellProof>>,
        change_foreign_shell_after_save: AtomicBool,
        change_foreign_shell_on_read: AtomicBool,
        change_record_token_on_read: AtomicBool,
        add_null_shell_identity_on_read: AtomicBool,
        replace_record_directory_on_read: AtomicBool,
        replace_after_empty_panes: AtomicBool,
        fail_read: AtomicBool,
        fail_shell_proof: AtomicBool,
        shell_proof_mutation: AtomicU64,
        oversized_read: AtomicBool,
        non_ascii_read: AtomicBool,
        restart_foreign_on_read: AtomicBool,
        leave_idle_shell_on_read: AtomicBool,
        wrong_foreign_cwd: AtomicBool,
    }
    impl Fake {
        fn pane(id: &str) -> Pane {
            Pane {
                pane_id: id.to_owned(),
                tab_id: "tab".to_owned(),
                workspace_id: "workspace".to_owned(),
                terminal_id: Some(format!("terminal-{id}")),
            }
        }

        fn custom_identity() -> CustomProcessIdentity {
            CustomProcessIdentity {
                version: 1,
                boot_id: "11111111-2222-3333-4444-555555555555".to_owned(),
                pid: 4242,
                starttime_ticks: 9001,
                executable_device: 7,
                executable_inode: 11,
            }
        }

        fn foreign_shell_identity() -> CustomProcessIdentity {
            CustomProcessIdentity {
                version: 1,
                boot_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned(),
                pid: 100,
                starttime_ticks: 8001,
                executable_device: 13,
                executable_inode: 17,
            }
        }
    }
    impl AgentApi for Fake {
        fn panes(&self) -> AdapterResult<Vec<Pane>> {
            self.panes_calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_panes.load(Ordering::Relaxed) {
                return Err(AdapterError::unavailable(
                    "pane query failed after allocation",
                ));
            }
            let snapshot = self.panes.lock().unwrap().clone();
            if snapshot.is_empty()
                && self
                    .replace_after_empty_panes
                    .swap(false, Ordering::Relaxed)
            {
                let mut replacement = Fake::pane("owned");
                replacement.terminal_id = Some("replacement-after-absence".to_owned());
                self.panes.lock().unwrap().push(replacement);
            }
            Ok(snapshot)
        }
        fn pane_info(&self, pane: &str) -> AdapterResult<AgentPaneInfo> {
            self.pane_info_calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_panes.load(Ordering::Relaxed) {
                return Err(AdapterError::unavailable(
                    "pane query failed after allocation",
                ));
            }
            if self.require_start_lock.load(Ordering::Relaxed) {
                let lock = agent::open_private_lock(
                    &self.root.join("registry/.worker.lock"),
                    "test lifecycle lock",
                )
                .unwrap();
                assert!(
                    FileExt::try_lock_exclusive(&lock).is_err(),
                    "startup released its generation lock before completion"
                );
            }
            if !self.panes.lock().unwrap().iter().any(|p| p.pane_id == pane) {
                return Err(AdapterError::unavailable("missing pane"));
            }
            let changed_after_save = self.change_session_after_save.load(Ordering::Relaxed)
                && self.root.join("registry/foreign/agent.json").exists();
            let owned_changed_after_save =
                self.change_owned_session_after_save.load(Ordering::Relaxed)
                    && agent::read_private_json(&self.root.join("registry/worker/agent.json"))
                        .ok()
                        .and_then(|record| record["lifecycle"].as_str().map(str::to_owned))
                        .as_deref()
                        == Some("running");
            let changed_after_save = changed_after_save || owned_changed_after_save;
            let report_session = self.report_session.load(Ordering::Relaxed)
                || changed_after_save
                || pane == "reported";
            let kind = if self.custom_reported.load(Ordering::Relaxed) {
                "muse"
            } else if pane == "claude" || self.claude_background.load(Ordering::Relaxed) {
                "claude"
            } else {
                "codex"
            };
            let pane_workspace = self
                .panes
                .lock()
                .unwrap()
                .iter()
                .find(|item| item.pane_id == pane)
                .map(|item| item.workspace_id.clone())
                .ok_or_else(|| AdapterError::unavailable("missing pane"))?;
            Ok(AgentPaneInfo {
                pane_id: pane.to_owned(),
                workspace_id: pane_workspace,
                cwd: if self.wrong_foreign_cwd.load(Ordering::Relaxed) {
                    self.root.join("other").display().to_string()
                } else {
                    self.root.display().to_string()
                },
                agent: self
                    .started
                    .load(Ordering::Relaxed)
                    .then(|| kind.to_owned()),
                status: if self.custom_native_working.load(Ordering::Relaxed) {
                    "working"
                } else {
                    "idle"
                }
                .to_owned(),
                session_agent: report_session.then(|| kind.to_owned()),
                session_value: report_session.then(|| {
                    if changed_after_save {
                        "replacement-thread"
                    } else if pane == *self.current_owned_pane.lock().unwrap()
                        || matches!(pane, "claude" | "reported")
                        || self.duplicate_session.load(Ordering::Relaxed)
                    {
                        "thread"
                    } else {
                        "human-thread"
                    }
                    .to_owned()
                }),
            })
        }
        fn workspace_label(&self, _: &str) -> AdapterResult<String> {
            Ok("subagents".to_owned())
        }
        fn run(&self, _: &str, text: &str) -> AdapterResult<()> {
            self.runs.lock().unwrap().push(text.to_owned());
            if let Some(screen) = self.screen_after_run.lock().unwrap().take() {
                *self.custom_screen.lock().unwrap() = Some(screen);
            }
            if self.working_after_run.load(Ordering::Relaxed) {
                self.custom_native_working.store(true, Ordering::Relaxed);
            }
            Ok(())
        }
        fn wait_agent_status(&self, _: &str, _: &str, _: u64) -> AdapterResult<()> {
            if self.wait_fails.load(Ordering::Relaxed) {
                Err(AdapterError::unavailable(
                    "simulated lost Herdr status event",
                ))
            } else {
                Ok(())
            }
        }
        fn read(&self, _: &str, _: &str, _: Option<usize>) -> AdapterResult<String> {
            if self.fail_read.load(Ordering::Relaxed) {
                return Err(AdapterError::unavailable("capture failed"));
            }
            if self.oversized_read.load(Ordering::Relaxed) {
                return Ok("\0".repeat(3 << 20));
            }
            if self.non_ascii_read.load(Ordering::Relaxed) {
                return Ok("é".repeat(3 << 20));
            }
            if self.add_sibling_on_read.swap(false, Ordering::Relaxed) {
                self.panes.lock().unwrap().push(Self::pane("human"));
            }
            if self.restart_foreign_on_read.swap(false, Ordering::Relaxed) {
                self.started.store(true, Ordering::Relaxed);
                self.report_session.store(true, Ordering::Relaxed);
            }
            if self.leave_idle_shell_on_read.swap(false, Ordering::Relaxed) {
                self.custom_at_idle_shell.store(false, Ordering::Relaxed);
            }
            if let Some(screen) = self.custom_screen.lock().unwrap().clone() {
                return Ok(screen);
            }
            if self
                .change_foreign_shell_on_read
                .swap(false, Ordering::Relaxed)
            {
                self.foreign_shell_identity.lock().unwrap().starttime_ticks += 1;
            }
            if let Some(replacement) = self.replacement_shell_on_read.lock().unwrap().take() {
                *self.foreign_shell_identity.lock().unwrap() = replacement.identity;
                *self.foreign_shell_path.lock().unwrap() = replacement.executable_path;
            }
            if self
                .change_record_token_on_read
                .swap(false, Ordering::Relaxed)
            {
                for name in ["foreign", "worker"] {
                    let path = self.root.join(format!("registry/{name}/agent.json"));
                    if path.is_file() {
                        let mut document = agent::read_private_json(&path).unwrap();
                        document["token"] = json!("replacement-generation");
                        agent::atomic_json(&path, &document).unwrap();
                    }
                }
            }
            if self
                .add_null_shell_identity_on_read
                .swap(false, Ordering::Relaxed)
            {
                let path = self.root.join("registry/foreign/agent.json");
                if path.is_file() {
                    let mut document = agent::read_private_json(&path).unwrap();
                    document["foreign_shell_identity"] = Value::Null;
                    agent::atomic_json(&path, &document).unwrap();
                }
            }
            if self
                .replace_record_directory_on_read
                .swap(false, Ordering::Relaxed)
            {
                for name in ["foreign", "worker"] {
                    let active = self.root.join(format!("registry/{name}"));
                    if active.is_dir() {
                        let displaced = self.root.join(format!("registry/.{name}-displaced"));
                        fs::rename(&active, &displaced).unwrap();
                        fs::create_dir(&active).unwrap();
                        fs::set_permissions(&active, fs::Permissions::from_mode(0o700)).unwrap();
                        fs::copy(displaced.join("agent.json"), active.join("agent.json")).unwrap();
                        fs::set_permissions(
                            active.join("agent.json"),
                            fs::Permissions::from_mode(0o600),
                        )
                        .unwrap();
                    }
                }
            }
            if self.custom_alive.load(Ordering::Relaxed) {
                if self.custom_ready.load(Ordering::Relaxed) {
                    Ok(
                        "Muse Code at Meta\n  Muse Code 1.4.0\n────────────────\n❯\n────────────────\nkiki · xhigh · /work · YOLO\n"
                            .to_owned(),
                    )
                } else {
                    Ok("Muse Code\nWorking…\n".to_owned())
                }
            } else if self.claude_background.load(Ordering::Relaxed) {
                Ok("✻ Waiting for 1 background agent to finish\n\
                     ────────────────────────────────────────\n❯\n\
                     ────────────────────────────────────────\n\
                     auto mode on · ← 2 agents · ↓ to manage\n\
                     ● main\n◯ reviewer Checking tests 8m\n"
                    .to_owned())
            } else {
                Ok("visible output".to_owned())
            }
        }
    }
    impl ManagedApi for Fake {
        fn workspace_id_for_label(&self, _: &str) -> AdapterResult<Option<String>> {
            if self.ambiguous_workspace.load(Ordering::Relaxed) {
                return Err(AdapterError::unavailable("ambiguous workspace label"));
            }
            Ok(Some("workspace".to_owned()))
        }
        fn create_workspace(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> AdapterResult<(String, String, String)> {
            unreachable!()
        }
        fn create_tab(
            &self,
            workspace: &str,
            label: &str,
            cwd: &str,
            environment: &[String],
        ) -> AdapterResult<String> {
            self.create_tab_with_pane(workspace, label, cwd, environment)
                .map(|value| value.0)
        }
        fn create_tab_with_pane(
            &self,
            _: &str,
            _: &str,
            _: &str,
            environment: &[String],
        ) -> AdapterResult<(String, String)> {
            self.environments.lock().unwrap().push(environment.to_vec());
            self.panes.lock().unwrap().push(Self::pane("owned"));
            Ok(("tab".to_owned(), "owned".to_owned()))
        }
        fn move_pane_to_new_tab(
            &self,
            pane: &str,
            expected_terminal: &str,
            workspace: &str,
            _: &str,
        ) -> AdapterResult<PaneMove> {
            let mut panes = self.panes.lock().unwrap();
            if self
                .replace_terminal_before_move
                .swap(false, Ordering::Relaxed)
            {
                panes
                    .iter_mut()
                    .find(|item| item.pane_id == pane)
                    .expect("replacement pane")
                    .terminal_id = Some("replacement-terminal".to_owned());
            }
            let old = panes
                .iter()
                .find(|item| item.pane_id == pane)
                .cloned()
                .ok_or_else(|| AdapterError::unavailable("missing pane"))?;
            if old.terminal_id.as_deref() != Some(expected_terminal) {
                return Err(AdapterError::unavailable(
                    "conditional terminal identity changed",
                ));
            }
            self.move_count.fetch_add(1, Ordering::Relaxed);
            let moved = Pane {
                pane_id: format!("{workspace}:moved"),
                tab_id: format!("{workspace}:tab"),
                workspace_id: workspace.to_owned(),
                terminal_id: old.terminal_id.clone(),
            };
            *panes.iter_mut().find(|item| item.pane_id == pane).unwrap() = moved.clone();
            *self.current_owned_pane.lock().unwrap() = moved.pane_id.clone();
            if self.fail_after_move.swap(false, Ordering::Relaxed) {
                return Err(AdapterError::unavailable(
                    "simulated lost pane move response",
                ));
            }
            Ok(PaneMove {
                previous_pane_id: if self.wrong_move_response.load(Ordering::Relaxed) {
                    "wrong".to_owned()
                } else {
                    old.pane_id
                },
                previous_tab_id: old.tab_id,
                previous_workspace_id: old.workspace_id,
                pane: moved,
            })
        }
        fn close_pane(&self, pane: &str) -> AdapterResult<()> {
            if self.fail_close.load(Ordering::Relaxed) {
                return Err(AdapterError::unavailable("close failed"));
            }
            self.closed.lock().unwrap().push(pane.to_owned());
            self.panes.lock().unwrap().retain(|p| p.pane_id != pane);
            if self.fail_after_close.swap(false, Ordering::Relaxed) {
                return Err(AdapterError::unavailable(
                    "injected result loss after close",
                ));
            }
            Ok(())
        }
        fn focus_pane(&self, pane: &str) -> AdapterResult<()> {
            self.focused.lock().unwrap().push(pane.to_owned());
            Ok(())
        }
        fn rename_tab(&self, _: &str, _: &str) -> AdapterResult<()> {
            Ok(())
        }
        fn start_agent(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &[String],
            _: Duration,
        ) -> AdapterResult<()> {
            if self.require_start_lock.load(Ordering::Relaxed) {
                let identity = agent::open_private_lock(
                    &self.root.join("registry/.identity.lock"),
                    "test identity lock",
                )
                .unwrap();
                assert!(
                    FileExt::try_lock_exclusive(&identity).is_err(),
                    "startup released its identity lock before native session commit"
                );
            }
            self.started.store(true, Ordering::Relaxed);
            Ok(())
        }
        fn start_pane_agent(
            &self,
            _: &str,
            _: &str,
            _: &str,
            args: &[String],
            _: Duration,
            persist: &mut dyn FnMut(CustomLaunchObservation) -> AdapterResult<()>,
        ) -> AdapterResult<()> {
            self.started.store(true, Ordering::Relaxed);
            self.custom_alive.store(true, Ordering::Relaxed);
            persist(CustomLaunchObservation::Intent {
                executable: PathBuf::from("/opt/agentctl/muse"),
                device: Self::custom_identity().executable_device,
                inode: Self::custom_identity().executable_inode,
                argv: std::iter::once("/opt/agentctl/muse".to_owned())
                    .chain(args.iter().cloned())
                    .collect(),
            })?;
            persist(CustomLaunchObservation::Process(Self::custom_identity()))?;
            if self.custom_fails_after_identity.load(Ordering::Relaxed) {
                let record =
                    agent::read_private_json(&self.root.join("registry/worker/agent.json"))
                        .expect("identity callback persisted the starting record");
                assert_eq!(record["lifecycle"], "starting");
                assert_eq!(record["custom_process_identity"]["pid"], 4242);
                return Err(AdapterError::unavailable(
                    "Muse workspace trust prompt requires human attention; no input was submitted",
                ));
            }
            self.custom_reported.store(true, Ordering::Relaxed);
            self.custom_alive.store(
                !self.custom_dies_after_report.load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
            Ok(())
        }
        fn recover_pane_agent(
            &self,
            _: &str,
            expected_argv: &[String],
            expected_device: u64,
            expected_inode: u64,
            expected_pid: u64,
        ) -> AdapterResult<CustomProcessIdentity> {
            if matches!(
                expected_argv.first().map(String::as_str),
                Some("/opt/agentctl/muse" | "muse")
            ) && expected_device == Self::custom_identity().executable_device
                && expected_inode == Self::custom_identity().executable_inode
                && expected_pid == Self::custom_identity().pid
                && self.custom_alive.load(Ordering::Relaxed)
            {
                Ok(Self::custom_identity())
            } else {
                Err(AdapterError::unavailable(
                    "custom harness recovery did not match",
                ))
            }
        }
        fn commit_recovered_pane_agent(
            &self,
            pane: &str,
            harness: &str,
            identity: &CustomProcessIdentity,
            commit: &mut dyn FnMut() -> AdapterResult<()>,
        ) -> AdapterResult<()> {
            self.verify_custom_harness(pane, harness, Some(identity))?;
            commit()?;
            if self
                .custom_dies_during_recovery_commit
                .swap(false, Ordering::Relaxed)
            {
                self.custom_alive.store(false, Ordering::Relaxed);
            }
            self.verify_custom_harness(pane, harness, Some(identity))
        }
        fn verify_custom_harness(
            &self,
            _: &str,
            _: &str,
            identity: Option<&CustomProcessIdentity>,
        ) -> AdapterResult<()> {
            self.custom_verify_calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_custom_verify_once.swap(false, Ordering::Relaxed) {
                return Err(AdapterError::unavailable("transient process-info response"));
            }
            if self.custom_alive.load(Ordering::Relaxed)
                && identity.is_none_or(|identity| identity == &Self::custom_identity())
            {
                Ok(())
            } else {
                Err(AdapterError::unavailable(
                    "custom harness is not the foreground process",
                ))
            }
        }
        fn process_generation_absent(
            &self,
            expected: &CustomProcessIdentity,
        ) -> AdapterResult<bool> {
            if expected != &Self::custom_identity() {
                return Ok(true);
            }
            Ok(!self.custom_alive.load(Ordering::Relaxed))
        }
        fn pane_is_idle_shell(&self, _: &str) -> AdapterResult<bool> {
            Ok(!self.custom_alive.load(Ordering::Relaxed)
                && self.custom_at_idle_shell.load(Ordering::Relaxed))
        }
        fn pane_shell_identity(&self, _: &str) -> AdapterResult<CustomProcessIdentity> {
            let mut identity = self.foreign_shell_identity.lock().unwrap().clone();
            if self.change_foreign_shell_after_save.load(Ordering::Relaxed)
                && self.root.join("registry/foreign/agent.json").exists()
            {
                identity.starttime_ticks += 1;
            }
            Ok(identity)
        }
        fn pane_is_same_idle_shell(
            &self,
            _: &str,
            expected: &CustomProcessIdentity,
        ) -> AdapterResult<bool> {
            Ok(!self.custom_alive.load(Ordering::Relaxed)
                && self.custom_at_idle_shell.load(Ordering::Relaxed)
                && *expected == *self.foreign_shell_identity.lock().unwrap())
        }
        fn pane_idle_shell_identity(&self, _: &str) -> AdapterResult<Option<PaneShellProof>> {
            if self.fail_shell_proof.load(Ordering::Relaxed) {
                return Err(AdapterError::unavailable("injected procfs proof failure"));
            }
            let proof = (!self.custom_alive.load(Ordering::Relaxed)
                && self.custom_at_idle_shell.load(Ordering::Relaxed))
            .then(|| PaneShellProof {
                identity: self.foreign_shell_identity.lock().unwrap().clone(),
                executable_path: self.foreign_shell_path.lock().unwrap().clone(),
            });
            if self.root.join("registry/worker/output.json").exists() {
                match self.shell_proof_mutation.swap(0, Ordering::Relaxed) {
                    1 => self.panes.lock().unwrap().push(Self::pane("human")),
                    2 => self.started.store(true, Ordering::Relaxed),
                    _ => {}
                }
            }
            Ok(proof)
        }
        fn agent_pane(&self, _: &str) -> AdapterResult<String> {
            Ok(self.current_owned_pane.lock().unwrap().clone())
        }
        fn report_agent_session(&self, _: &str, _: &str, _: &str, _: &str) -> AdapterResult<()> {
            Ok(())
        }
        fn send_text(&self, _: &str, text: &str) -> AdapterResult<()> {
            self.sent_texts.lock().unwrap().push(text.to_owned());
            Ok(())
        }
        fn send_keys(&self, _: &str, key: &str) -> AdapterResult<()> {
            self.sent_keys.lock().unwrap().push(key.to_owned());
            if matches!(key, "Enter" | "ctrl+x ctrl+s") {
                if self.ignore_first_enter.swap(false, Ordering::Relaxed) {
                    return Ok(());
                }
                if let Some(screen) = self.screen_after_enter.lock().unwrap().take() {
                    *self.custom_screen.lock().unwrap() = Some(screen);
                }
                self.custom_native_working.store(true, Ordering::Relaxed);
            }
            Ok(())
        }
        fn close_tab(&self, _: &str) -> AdapterResult<()> {
            panic!("managed lifecycle must close exactly the owned pane")
        }
    }

    #[test]
    fn startup_keeps_its_generation_lock_through_brief_and_returned_status() {
        let fixture = Fixture::new();
        fixture
            .client
            .require_start_lock
            .store(true, Ordering::Relaxed);
        let status = fixture.start(Some("initial task".to_owned()));
        assert_eq!(status["lifecycle"], "running");
        assert_eq!(*fixture.client.runs.lock().unwrap(), ["initial task"]);
    }

    #[test]
    fn relocation_preserves_generation_and_pending_queue() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let started = fixture.start(None);
        fixture
            .manager()
            .send("worker", "before relocation", DrainOptions::default())
            .unwrap();
        let queue = fixture.root.join("registry/worker/queue");
        let identifier = agent::enqueue(&queue, "retained prompt", Some("queued")).unwrap();
        let artifact = queue.join("inbox").join(format!("{identifier}.json"));
        let before = fs::read(&artifact).unwrap();

        let moved = fixture
            .manager()
            .relocate("worker", Some("destination"), None, true)
            .unwrap();

        assert_eq!(moved["workspace_id"], "destination");
        assert_eq!(moved["terminal_id"], "terminal-owned");
        let persisted = fixture.manager().load("worker").unwrap();
        assert_eq!(persisted.pane_id.as_deref(), Some("destination:moved"));
        assert_eq!(persisted.token, started["token"]);
        assert_eq!(fs::read(&artifact).unwrap(), before);
        fixture
            .manager()
            .send("worker", "after relocation", DrainOptions::default())
            .unwrap();
        assert_eq!(fixture.client.runs.lock().unwrap()[0], "before relocation");
        let mut after = fixture.client.runs.lock().unwrap()[1..].to_vec();
        after.sort();
        assert_eq!(after, ["after relocation", "retained prompt"]);
        let binding = agent::read_private_json(&queue.join("target.json")).unwrap();
        assert_eq!(binding["kind"], "pane");
        assert_eq!(binding["pane_id"], "destination:moved");
        assert!(!fixture
            .root
            .join("registry/worker/relocation.json")
            .exists());
        assert_eq!(fixture.client.move_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn relocation_reconciles_a_move_with_a_lost_or_wrong_response() {
        for wrong_response in [false, true] {
            let fixture = Fixture::new();
            fixture.start(None);
            if wrong_response {
                fixture
                    .client
                    .wrong_move_response
                    .store(true, Ordering::Relaxed);
            } else {
                fixture
                    .client
                    .fail_after_move
                    .store(true, Ordering::Relaxed);
            }

            let error = fixture
                .manager()
                .relocate("worker", Some("destination"), None, true)
                .unwrap_err();
            if wrong_response {
                assert!(error.to_string().contains("different source routing"));
                fixture
                    .client
                    .wrong_move_response
                    .store(false, Ordering::Relaxed);
            } else {
                assert!(error.to_string().contains("lost pane move response"));
            }
            assert!(fixture
                .root
                .join("registry/worker/relocation.json")
                .is_file());

            let recovered = fixture
                .manager()
                .relocate("worker", Some("destination"), None, true)
                .unwrap();
            assert_eq!(recovered["pane_id"], "destination:moved");
            assert_eq!(fixture.client.move_count.load(Ordering::Relaxed), 1);
            assert!(!fixture
                .root
                .join("registry/worker/relocation.json")
                .exists());
        }
    }

    #[test]
    fn relocation_reconciles_after_routing_commit_before_journal_removal() {
        let fixture = Fixture::new();
        let started = fixture.start(None);
        fixture
            .manager()
            .relocate("worker", Some("destination"), None, true)
            .unwrap();
        let journal = RelocationJournal {
            schema: RELOCATION_SCHEMA.to_owned(),
            token: started["token"].as_str().unwrap().to_owned(),
            terminal_id: "terminal-owned".to_owned(),
            old: PaneRoute {
                workspace_id: "workspace".to_owned(),
                tab_id: "tab".to_owned(),
                pane_id: "owned".to_owned(),
            },
            target_workspace_id: "destination".to_owned(),
            new_tab: true,
        };
        agent::atomic_json(
            &fixture.root.join("registry/worker/relocation.json"),
            &serde_json::to_value(journal).unwrap(),
        )
        .unwrap();

        let recovered = fixture
            .manager()
            .relocate("worker", Some("destination"), None, true)
            .unwrap();
        assert_eq!(recovered["pane_id"], "destination:moved");
        assert_eq!(fixture.client.move_count.load(Ordering::Relaxed), 1);
        assert!(!fixture
            .root
            .join("registry/worker/relocation.json")
            .exists());
    }

    #[test]
    fn relocation_refuses_multi_pane_and_ambiguous_workspace_before_move() {
        let fixture = Fixture::new();
        fixture.start(None);
        fixture.client.panes.lock().unwrap().push(Pane {
            pane_id: "human".to_owned(),
            tab_id: "tab".to_owned(),
            workspace_id: "workspace".to_owned(),
            terminal_id: Some("terminal-human".to_owned()),
        });
        let error = fixture
            .manager()
            .relocate("worker", Some("destination"), None, true)
            .unwrap_err();
        assert!(error.to_string().contains("multi-pane"));
        assert!(!fixture
            .root
            .join("registry/worker/relocation.json")
            .exists());
        fixture
            .client
            .panes
            .lock()
            .unwrap()
            .retain(|pane| pane.pane_id != "human");
        fixture
            .client
            .ambiguous_workspace
            .store(true, Ordering::Relaxed);
        let error = fixture
            .manager()
            .relocate("worker", None, Some("project"), true)
            .unwrap_err();
        assert!(error.to_string().contains("ambiguous workspace label"));
        assert_eq!(fixture.client.move_count.load(Ordering::Relaxed), 0);
        assert!(!fixture
            .root
            .join("registry/worker/relocation.json")
            .exists());
    }

    #[test]
    fn relocation_conditional_move_refuses_terminal_replacement_in_final_gap() {
        let fixture = Fixture::new();
        fixture.start(None);
        fixture
            .client
            .replace_terminal_before_move
            .store(true, Ordering::Relaxed);

        let error = fixture
            .manager()
            .relocate("worker", Some("destination"), None, true)
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("conditional terminal identity changed"));
        assert_eq!(fixture.client.move_count.load(Ordering::Relaxed), 0);
        assert_eq!(
            fixture.manager().load("worker").unwrap().pane_id.as_deref(),
            Some("owned")
        );
        assert!(fixture
            .root
            .join("registry/worker/relocation.json")
            .is_file());
    }

    #[test]
    fn health_isolates_dead_and_malformed_records_and_persists_detection() {
        let fixture = Fixture::new();
        fixture.start(None);
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let stale = fixture.root.join("registry/stale");
        fs::create_dir(&stale).unwrap();
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(stale.join("agent.json"), b"{}\n").unwrap();
        fs::set_permissions(stale.join("agent.json"), fs::Permissions::from_mode(0o600)).unwrap();

        let health = fixture
            .manager()
            .health(&["stale".to_owned(), "worker".to_owned()]);
        assert_eq!(health["healthy"], false);
        let sessions = health["sessions"].as_array().unwrap();
        let dead = sessions
            .iter()
            .find(|value| value["name"] == "worker")
            .unwrap();
        let stale = sessions
            .iter()
            .find(|value| value["name"] == "stale")
            .unwrap();
        assert_eq!(dead["health"], "unhealthy");
        assert_eq!(dead["reason_code"], "expected-harness-missing");
        assert!(dead["reason"]
            .as_str()
            .unwrap()
            .contains("reports agent None, expected 'codex'"));
        assert_eq!(stale["health"], "unknown");
        assert_eq!(stale["recorded"], false);
        let (rows, list_healthy) = fixture.manager().list_with_health();
        let stale_row = rows
            .iter()
            .find(|row| row["name"] == "stale")
            .expect("malformed row remains visible");
        assert!(!list_healthy);
        assert_eq!(rows.len(), 2);
        assert_eq!(stale_row["health"], "unknown");
        assert!(stale_row["probe_error"]
            .as_str()
            .unwrap()
            .contains("invalid agent record"));
        assert!(fixture.manager().status_with_health("stale").is_err());
        let durable =
            agent::read_private_json(&fixture.root.join("registry/worker/health.json")).unwrap();
        assert_eq!(durable["schema"], HEALTH_SCHEMA);
        assert_eq!(durable["last_unhealthy_reason"], dead["reason"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn list_quarantines_fifo_agent_record_without_blocking_other_rows() {
        let fixture = Fixture::new();
        fixture.start(None);
        let blocked = fixture.root.join("registry/blocked");
        fs::create_dir(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        let fifo = blocked.join("agent.json");
        let fifo_name = CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: fifo_name is a live NUL-terminated path and mkfifo does not
        // retain the pointer after returning.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);

        let started = Instant::now();
        let (rows, healthy) = fixture.manager().list_with_health();

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!healthy);
        assert_eq!(rows.len(), 2);
        let blocked = rows
            .iter()
            .find(|row| row["name"] == "blocked")
            .expect("FIFO row remains visible");
        assert_eq!(blocked["health"], "unknown");
        assert_eq!(blocked["health_reason_code"], "registry-or-health-error");
    }

    #[test]
    fn health_skips_contended_first_lock_and_probes_later_row() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let mut alpha = manager.load("worker").unwrap();
        alpha.name = "alpha".to_owned();
        alpha.token = "alpha-generation".to_owned();
        fs::create_dir(fixture.root.join("registry/alpha")).unwrap();
        fs::set_permissions(
            fixture.root.join("registry/alpha"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        manager.save(&alpha).unwrap();
        let held = agent::open_private_lock(
            &fixture.root.join("registry/.alpha.lock"),
            "held lifecycle lock",
        )
        .unwrap();
        held.lock_exclusive().unwrap();
        fixture.client.pane_info_calls.store(0, Ordering::Relaxed);
        fixture.client.panes_calls.store(0, Ordering::Relaxed);
        let started = Instant::now();
        let (health, _statuses) = manager.health_snapshot_until(
            &["alpha".to_owned(), "worker".to_owned()],
            Instant::now() + Duration::from_millis(250),
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(health["sessions"][0]["health"], "unknown");
        assert_eq!(
            health["sessions"][0]["reason_code"],
            "lifecycle-lock-contended"
        );
        assert_eq!(health["sessions"][0]["recorded"], false);
        assert_eq!(health["sessions"][1]["health"], "healthy");
        assert!(fixture.client.pane_info_calls.load(Ordering::Relaxed) > 0);
        assert!(fixture.client.panes_calls.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn health_transport_failure_is_unknown_not_proof_of_death() {
        let fixture = Fixture::new();
        fixture.start(None);
        fixture.client.fail_panes.store(true, Ordering::Relaxed);
        let health = fixture.manager().health(&["worker".to_owned()]);
        assert_eq!(health["healthy"], false);
        assert_eq!(health["sessions"][0]["health"], "unknown");
        assert_eq!(health["sessions"][0]["reason_code"], "runtime-probe-failed");
    }

    #[test]
    fn health_does_not_call_transient_agent_detector_failure_dead() {
        let fixture = Fixture::new();
        fixture.start(None);
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture
            .client
            .custom_at_idle_shell
            .store(false, Ordering::Relaxed);
        let health = fixture.manager().health(&["worker".to_owned()]);
        assert_eq!(health["healthy"], false);
        assert_eq!(health["sessions"][0]["health"], "unknown");
        assert_eq!(health["sessions"][0]["runtime_state"], "unknown");
        assert_eq!(health["sessions"][0]["reason_code"], "agent-report-missing");
    }

    #[test]
    fn health_does_not_call_live_muse_process_probe_failure_dead() {
        let fixture = Fixture::new();
        fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        fixture
            .client
            .fail_custom_verify_once
            .store(true, Ordering::Relaxed);
        fixture
            .client
            .custom_verify_calls
            .store(0, Ordering::Relaxed);
        let health = fixture.manager().health(&["worker".to_owned()]);
        assert!(fixture.client.custom_alive.load(Ordering::Relaxed));
        assert!(fixture.client.custom_reported.load(Ordering::Relaxed));
        assert_eq!(health["sessions"][0]["health"], "unknown");
        assert_eq!(health["sessions"][0]["runtime_state"], "unknown");
        assert_eq!(
            health["sessions"][0]["reason_code"],
            "custom-harness-verification-unconfirmed"
        );
        assert_eq!(
            fixture.client.custom_verify_calls.load(Ordering::Relaxed),
            2
        );
    }

    #[test]
    fn health_calls_stale_muse_label_dead_only_from_stable_shell_proof() {
        let fixture = Fixture::new();
        fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        assert!(fixture.client.custom_reported.load(Ordering::Relaxed));
        fixture.client.custom_alive.store(false, Ordering::Relaxed);
        fixture
            .client
            .custom_at_idle_shell
            .store(true, Ordering::Relaxed);
        let health = fixture.manager().health(&["worker".to_owned()]);
        assert!(fixture.client.custom_reported.load(Ordering::Relaxed));
        assert_eq!(health["sessions"][0]["health"], "unhealthy", "{health}");
        assert_eq!(health["sessions"][0]["runtime_state"], "dead");
        assert_eq!(
            health["sessions"][0]["reason_code"],
            "expected-harness-missing"
        );
        assert!(health["sessions"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("stable idle shell"));
    }

    #[test]
    fn health_does_not_call_reused_idle_shell_pane_this_generation_dead() {
        for mutation in ["workspace", "tab", "cwd", "session"] {
            let fixture = Fixture::new();
            fixture.start(None);
            fixture.client.started.store(false, Ordering::Relaxed);
            fixture
                .client
                .report_session
                .store(false, Ordering::Relaxed);
            match mutation {
                "workspace" => {
                    fixture.client.panes.lock().unwrap()[0].workspace_id = "replacement".to_owned();
                }
                "tab" => {
                    fixture.client.panes.lock().unwrap()[0].tab_id = "replacement".to_owned();
                }
                "cwd" => {
                    fs::create_dir(fixture.root.join("other")).unwrap();
                    fixture
                        .client
                        .wrong_foreign_cwd
                        .store(true, Ordering::Relaxed);
                }
                "session" => {
                    fixture
                        .client
                        .change_owned_session_after_save
                        .store(true, Ordering::Relaxed);
                }
                _ => unreachable!(),
            }
            let health = fixture.manager().health(&["worker".to_owned()]);
            assert_eq!(health["sessions"][0]["health"], "unhealthy", "{mutation}");
            assert_eq!(
                health["sessions"][0]["runtime_state"], "unknown",
                "{mutation}"
            );
            assert_eq!(
                health["sessions"][0]["reason_code"], "runtime-identity-mismatch",
                "{mutation}"
            );
        }
    }

    #[test]
    fn identityless_failed_muse_launch_requires_token_and_exact_pid_to_recover() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    harness_args: vec!["--literal".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap();
        assert_eq!(started["launch_executable"], "/opt/agentctl/muse");
        assert_eq!(
            started["launch_argv"],
            json!(["/opt/agentctl/muse", "--literal"])
        );
        assert_eq!(started["runtime_ownership"], "owned");
        let path = fixture.root.join("registry/worker/agent.json");
        let mut record = agent::read_private_json(&path).unwrap();
        record["lifecycle"] = json!("launch_failed");
        record["custom_process_identity"] = Value::Null;
        record["pane_reported_by_agentctl"] = json!(false);
        record["native_session"] = Value::Null;
        record["error"] = json!("transient process-info response");
        agent::atomic_json(&path, &record).unwrap();
        fixture
            .client
            .custom_reported
            .store(false, Ordering::Relaxed);
        fixture.client.started.store(false, Ordering::Relaxed);

        assert!(manager
            .recover_start(
                "worker",
                started["token"].as_str().unwrap(),
                Fake::custom_identity().pid + 1,
            )
            .unwrap_err()
            .to_string()
            .contains("did not match"));
        fixture
            .client
            .custom_dies_during_recovery_commit
            .store(true, Ordering::Relaxed);
        assert!(manager
            .recover_start(
                "worker",
                started["token"].as_str().unwrap(),
                Fake::custom_identity().pid,
            )
            .unwrap_err()
            .to_string()
            .contains("after identity persistence"));
        let partial = agent::read_private_json(&path).unwrap();
        assert_eq!(partial["lifecycle"], "launch_failed");
        assert_eq!(partial["custom_process_identity"]["pid"], 4242);
        assert_eq!(partial["pane_reported_by_agentctl"], false);
        fixture.client.custom_alive.store(true, Ordering::Relaxed);
        // Legacy starts saved the exact observed process but not the launch
        // executable fields.  Preserve that process identity as the image
        // authority and reconstruct argv from the legacy arguments.
        let mut legacy = manager.load("worker").unwrap().public_value();
        let legacy = legacy.as_object_mut().unwrap();
        for field in [
            "launch_profile",
            "launch_executable",
            "launch_executable_device",
            "launch_executable_inode",
            "launch_argv",
            "launch_environment_names",
            "runtime_ownership",
            "runner_pid",
            "runner_started_at",
            "runner_identity",
        ] {
            legacy.remove(field);
        }
        legacy.insert("schema".to_owned(), json!(1));
        agent::atomic_json(&path, &Value::Object(legacy.clone())).unwrap();
        let recovered = manager
            .recover_start(
                "worker",
                started["token"].as_str().unwrap(),
                Fake::custom_identity().pid,
            )
            .unwrap();
        assert_eq!(recovered["lifecycle"], "running");
        assert!(recovered["probe_error"].is_null());
        assert_eq!(recovered["custom_process_identity"]["pid"], 4242);
        assert_eq!(recovered["pane_reported_by_agentctl"], false);
        assert!(fixture.client.runs.lock().unwrap().is_empty());
    }

    #[test]
    fn failed_muse_recovery_does_not_report_idle_without_idle_composer() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut record = agent::read_private_json(&path).unwrap();
        record["lifecycle"] = json!("launch_failed");
        record["custom_process_identity"] = Value::Null;
        record["pane_reported_by_agentctl"] = json!(false);
        record["native_session"] = Value::Null;
        record["error"] = json!("transient process-info response");
        agent::atomic_json(&path, &record).unwrap();
        fixture
            .client
            .custom_reported
            .store(false, Ordering::Relaxed);
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture.client.custom_ready.store(false, Ordering::Relaxed);

        let error = manager
            .recover_start(
                "worker",
                started["token"].as_str().unwrap(),
                Fake::custom_identity().pid,
            )
            .unwrap_err();
        assert!(error.to_string().contains("no verified idle composer"));
        let partial = agent::read_private_json(&path).unwrap();
        assert_eq!(partial["lifecycle"], "launch_failed");
        assert_eq!(partial["pane_reported_by_agentctl"], false);
        assert_eq!(partial["custom_process_identity"]["pid"], 4242);
        assert!(partial["error"]
            .as_str()
            .unwrap()
            .contains("no verified idle composer"));
        assert!(!fixture.client.custom_reported.load(Ordering::Relaxed));
        assert!(fixture.client.runs.lock().unwrap().is_empty());
    }

    #[test]
    fn custom_process_identity_outweighs_a_missing_native_agent_label() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let mut record = manager.load("worker").unwrap();
        record.session_agent = None;
        record.session_value = None;
        record.session_source = None;
        manager.save(&record).unwrap();
        fixture
            .client
            .custom_reported
            .store(false, Ordering::Relaxed);

        let status = manager.status("worker").unwrap();

        assert_eq!(
            started["custom_process_identity"],
            status["custom_process_identity"]
        );
        assert_eq!(status["agent"], "muse", "{status}");
        assert_eq!(status["agent_status"], "idle");
        assert!(status["probe_error"].is_null());
    }

    #[test]
    fn headerless_muse_idle_requires_matching_terminal_status() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        *fixture.client.custom_screen.lock().unwrap() = Some(
            "old transcript after the version header scrolled away\n\
             ────────────────\n❯\n────────────────\n\
             kiki · xhigh · /work · YOLO\n"
                .to_owned(),
        );
        assert_eq!(manager.status("worker").unwrap()["agent_status"], "idle");

        *fixture.client.custom_screen.lock().unwrap() = Some(
            "old transcript after the version header scrolled away\n\
             ────────────────\n❯ queued owner prompt\n────────────────\n\
             kiki · xhigh · /work · YOLO\n"
                .to_owned(),
        );
        assert_eq!(manager.status("worker").unwrap()["agent_status"], "staged");

        fixture
            .client
            .custom_native_working
            .store(true, Ordering::Relaxed);
        assert_eq!(manager.status("worker").unwrap()["agent_status"], "working");
    }

    #[test]
    fn muse_submits_matching_prebuffered_prompt_once_without_reinjection() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let divider = "────────────────────────────────────────";
        let prompt =
            "Use deterministic-scheduling-review and preserve the exact staged owner prompt";
        *fixture.client.custom_screen.lock().unwrap() = Some(format!(
            "old transcript\n{divider}\n❯ Use deterministic-\n  scheduling-review and preserve the exact staged owner prompt\n{divider}\nkiki · xhigh · /work · YOLO\n"
        ));
        *fixture.client.screen_after_enter.lock().unwrap() = Some(format!(
            "❯ Use deterministic-\n  scheduling-review and preserve the exact staged owner prompt\n◆ Working\n{divider}\n❯\n{divider}\nkiki · xhigh · /work · YOLO\n"
        ));

        assert_eq!(manager.status("worker").unwrap()["agent_status"], "staged");
        let result = manager
            .send("worker", prompt, DrainOptions::default())
            .unwrap();

        assert_eq!(result.outcome, agent::QueueOutcome::Delivered);
        assert!(fixture.client.sent_texts.lock().unwrap().is_empty());
        assert_eq!(*fixture.client.sent_keys.lock().unwrap(), ["Enter"]);
    }

    #[test]
    fn claude_staged_prompt_uses_exact_submit_chord_once() {
        let fixture = Fixture::new();
        fixture
            .client
            .claude_background
            .store(true, Ordering::Relaxed);
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "claude".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let divider = "────────────────────────────────────────";
        let prompt = "inspect the exact state and continue safely";
        *fixture.client.custom_screen.lock().unwrap() =
            Some(format!("{divider}\n❯\n{divider}\nauto mode on\n"));
        *fixture.client.screen_after_run.lock().unwrap() = Some(format!(
            "{divider}\n❯ {prompt}\n  ctrl+x ctrl+s to send now\n\
             {divider}\n⏵⏵ auto mode on · esc to interrupt\n"
        ));
        *fixture.client.screen_after_enter.lock().unwrap() = Some(format!(
            "❯ {prompt}\n● Thinking\n{divider}\n❯\n{divider}\n\
             ⏵⏵ auto mode on · esc to interrupt\n"
        ));

        let result = manager
            .send("worker", prompt, DrainOptions::default())
            .unwrap();

        assert_eq!(result.outcome, agent::QueueOutcome::Delivered);
        assert_eq!(*fixture.client.runs.lock().unwrap(), [prompt]);
        assert_eq!(*fixture.client.sent_keys.lock().unwrap(), ["ctrl+x ctrl+s"]);
    }

    #[test]
    fn claude_new_active_ui_confirms_submission_when_long_turn_left_history() {
        let fixture = Fixture::new();
        fixture
            .client
            .claude_background
            .store(true, Ordering::Relaxed);
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "claude".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let divider = "────────────────────────────────────────";
        let prompt = "review the exact long prompt whose rendered turn left bounded history";
        *fixture.client.custom_screen.lock().unwrap() =
            Some(format!("{divider}\n❯\n{divider}\nauto mode on\n"));
        *fixture.client.screen_after_run.lock().unwrap() = Some(format!(
            "{divider}\n❯ {prompt}\n  ctrl+x ctrl+s to send now\n\
             {divider}\nauto mode on\n"
        ));
        *fixture.client.screen_after_enter.lock().unwrap() = Some(format!(
            "● Inspecting repository\n{divider}\n❯\n{divider}\n\
             ⏵⏵ auto mode on · esc to interrupt\n"
        ));

        let result = manager
            .send("worker", prompt, DrainOptions::default())
            .unwrap();

        assert_eq!(result.outcome, agent::QueueOutcome::Delivered);
        assert_eq!(*fixture.client.sent_keys.lock().unwrap(), ["ctrl+x ctrl+s"]);
    }

    #[test]
    fn claude_different_staged_prompt_is_not_overwritten_or_submitted() {
        let fixture = Fixture::new();
        fixture
            .client
            .claude_background
            .store(true, Ordering::Relaxed);
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "claude".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let divider = "────────────────────────────────────────";
        *fixture.client.custom_screen.lock().unwrap() = Some(format!(
            "{divider}\n❯ existing owner prompt\n  ctrl+x ctrl+s to send now\n\
             {divider}\nauto mode on\n"
        ));

        assert_eq!(manager.status("worker").unwrap()["agent_status"], "staged");
        let error = manager
            .send("worker", "different queued prompt", DrainOptions::default())
            .unwrap_err();
        assert!(error.to_string().contains("different buffered input"));
        assert!(fixture.client.runs.lock().unwrap().is_empty());
        assert!(fixture.client.sent_keys.lock().unwrap().is_empty());
    }

    #[test]
    fn muse_retries_enter_once_only_while_exact_prompt_remains_staged() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let divider = "────────────────────────────────────────";
        let prompt = "continue the exact queued task after the first Enter was ignored";
        *fixture.client.custom_screen.lock().unwrap() = Some(format!(
            "old transcript\n{divider}\n❯ {prompt}\n{divider}\nkiki · xhigh · /work · YOLO\n"
        ));
        *fixture.client.screen_after_enter.lock().unwrap() = Some(format!(
            "❯ {prompt}\n◆ Thinking\n{divider}\n❯\n{divider}\nkiki · xhigh · /work · YOLO\n"
        ));
        fixture
            .client
            .ignore_first_enter
            .store(true, Ordering::Relaxed);

        let result = manager
            .send("worker", prompt, DrainOptions::default())
            .unwrap();

        assert_eq!(result.outcome, agent::QueueOutcome::Delivered);
        assert!(fixture.client.sent_texts.lock().unwrap().is_empty());
        assert_eq!(
            *fixture.client.sent_keys.lock().unwrap(),
            ["Enter", "Enter"]
        );
    }

    #[test]
    fn muse_refuses_different_prebuffered_prompt_without_terminal_input() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        *fixture.client.custom_screen.lock().unwrap() = Some(
            "old transcript\n────────────────\n❯ human draft that must be preserved\n────────────────\nkiki · xhigh · /work · YOLO\n"
                .to_owned(),
        );

        assert_eq!(manager.status("worker").unwrap()["agent_status"], "staged");
        let error = manager
            .send(
                "worker",
                "queued automation prompt",
                DrainOptions::default(),
            )
            .unwrap_err();
        assert!(error.to_string().contains("different buffered input"));
        assert!(fixture.client.sent_texts.lock().unwrap().is_empty());
        assert!(fixture.client.sent_keys.lock().unwrap().is_empty());
    }

    #[test]
    fn ambiguous_muse_delivery_reconciles_only_an_exact_user_turn() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        manager.drain("worker", DrainOptions::default()).unwrap();
        let queue = fixture.root.join("registry/worker/queue");
        let identifier = "collapsed-goal";
        let prompt = format!("long literal goal {}suffix", "middle ".repeat(180));
        agent::enqueue(&queue, &prompt, Some(identifier)).unwrap();
        let inbox = queue.join("inbox").join(format!("{identifier}.json"));
        let mut document = agent::read_private_json(&inbox).unwrap();
        document["possibly_submitted"] = json!(true);
        document["delivery_state"] = json!("inflight");
        agent::atomic_json(&inbox, &document).unwrap();
        let failed = queue.join("failed").join(format!("{identifier}.json"));
        fs::rename(&inbox, &failed).unwrap();
        agent::sync_directory(&queue.join("inbox")).unwrap();
        agent::sync_directory(&queue.join("failed")).unwrap();
        agent::atomic_json(
            &queue
                .join("failed")
                .join(format!("{identifier}.json.error")),
            &json!({"outcome":"possibly_submitted"}),
        )
        .unwrap();
        let artifact = fs::read(&failed).unwrap();
        let digest = format!("{:x}", Sha256::digest(&artifact));
        let header = "Muse Code 1.4.0\n";
        let divider = "────────────────\n";
        let footer = "kiki · xhigh · /work · YOLO\n";

        *fixture.client.custom_screen.lock().unwrap() =
            Some(format!("{header}◆ {prompt}\n{divider}❯\n{divider}{footer}"));
        assert!(manager
            .reconcile_delivery("worker", identifier, &digest)
            .unwrap_err()
            .to_string()
            .contains("exact prompt as a Muse user turn"));
        assert!(manager
            .reconcile_delivery("worker", identifier, &"0".repeat(64))
            .unwrap_err()
            .to_string()
            .contains("expected-sha256"));

        *fixture.client.custom_screen.lock().unwrap() = Some(format!(
            "{header}❯ {prompt}\n◆ Working\n{divider}❯ {prompt}\n{divider}{footer}"
        ));
        assert!(manager
            .reconcile_delivery("worker", identifier, &digest)
            .unwrap_err()
            .to_string()
            .contains("exact prompt as a Muse user turn"));

        *fixture.client.custom_screen.lock().unwrap() = Some(format!(
            "{header}❯ {prompt}\n◆ Working\n{divider}❯\n{divider}{footer}"
        ));
        let reconciled = manager
            .reconcile_delivery("worker", identifier, &digest)
            .unwrap();
        assert_eq!(reconciled.outcome, agent::QueueOutcome::Delivered);
        let processed = queue.join("processed").join(format!("{identifier}.json"));
        assert_eq!(fs::read(&processed).unwrap(), artifact);
        assert!(!failed.exists());
        assert!(!queue
            .join("failed")
            .join(format!("{identifier}.json.error"))
            .exists());
        assert_eq!(
            manager
                .reconcile_delivery("worker", identifier, &digest)
                .unwrap()
                .outcome,
            agent::QueueOutcome::Delivered
        );
    }

    #[test]
    fn status_does_not_call_claude_idle_while_background_agent_is_working() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        fixture
            .client
            .claude_background
            .store(true, Ordering::Relaxed);
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "claude".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let screen = fixture.client.read("owned", "visible", Some(200)).unwrap();
        assert!(claude_active_screen(&screen), "{screen:?}");
        assert_eq!(
            fixture.client.pane_info("owned").unwrap().agent.as_deref(),
            Some("claude")
        );
        let status = manager.status("worker").unwrap();

        assert_eq!(status["agent_status"], "working");
        assert!(status["probe_error"].is_null());
    }

    #[test]
    fn environment_entries_preserve_literals_and_reject_invalid_input() {
        let entries = vec![
            "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai".to_owned(),
            "LITERAL= spaces $(unexpanded) = remain ".to_owned(),
            "UNICODE=snowman-☃\nnext-line".to_owned(),
            "EMPTY=".to_owned(),
        ];
        assert_eq!(environment_entries(&entries).unwrap(), entries);
        for invalid in [
            "MISSING_EQUALS",
            "=value",
            "9START=value",
            "BAD-NAME=value",
            "BAD\0NAME=value",
            "GOOD=bad\0value",
        ] {
            assert!(
                environment_entries(&[invalid.to_owned()]).is_err(),
                "accepted invalid environment entry {invalid:?}"
            );
        }
    }

    #[test]
    fn start_sets_environment_only_on_the_created_tab() {
        let fixture = Fixture::new();
        let entries = vec![
            "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai".to_owned(),
            "LITERAL=a b=$(unexpanded)=tail".to_owned(),
        ];
        let status = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    environment: entries.clone(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let observed = fixture.client.environments.lock().unwrap();
        assert_eq!(observed.as_slice(), std::slice::from_ref(&entries));
        drop(observed);
        assert!(status.get("environment").is_none());
        assert_eq!(
            status["launch_environment_names"],
            json!(["META_CODEX_AI_GATEWAY", "LITERAL"])
        );
        assert_eq!(status["runtime_ownership"], "owned");
        for entry in entries {
            assert!(!status.to_string().contains(&entry));
        }
    }

    #[test]
    fn invalid_environment_is_refused_before_registry_or_tab_creation() {
        let fixture = Fixture::new();
        let error = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    environment: vec!["BAD-NAME=value".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("environment variable name"));
        assert!(!fixture.root.join("registry").exists());
        assert!(fixture.client.environments.lock().unwrap().is_empty());
    }

    #[test]
    fn interactive_muse_refuses_raw_duplicate_structured_options_before_allocation() {
        let fixture = Fixture::new();
        let model_error = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    harness: "muse".to_owned(),
                    model: Some("structured".to_owned()),
                    harness_args: vec!["--model".to_owned(), "raw".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(model_error.to_string().contains("model field"));
        let effort_error = fixture
            .manager()
            .start_with_reasoning_effort(
                "worker",
                &fixture.root,
                "ultra",
                StartOptions {
                    harness: "muse".to_owned(),
                    harness_args: vec!["--reasoning-effort=low".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(effort_error.to_string().contains("reasoning effort"));
        assert!(!fixture.root.join("registry").exists());
        assert!(!fixture.client.started.load(Ordering::Relaxed));
        assert!(fixture.client.environments.lock().unwrap().is_empty());
    }

    #[test]
    fn direct_start_preserves_unstructured_raw_policy_and_nonoverriding_codex_config() {
        let unstructured = Fixture::new();
        let unstructured_status = unstructured
            .manager()
            .start(
                "worker",
                &unstructured.root,
                StartOptions {
                    harness_args: vec!["--reasoning-effort=high".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap();
        assert_eq!(
            unstructured_status["arguments"],
            json!(["--no-alt-screen", "--reasoning-effort=high"])
        );

        let codex = Fixture::new();
        let codex_status = codex
            .manager()
            .start(
                "worker",
                &codex.root,
                StartOptions {
                    model: Some("structured".to_owned()),
                    harness_args: vec!["-c".to_owned(), "sandbox_mode=read-only".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap();
        assert_eq!(
            codex_status["arguments"],
            json!([
                "--no-alt-screen",
                "--model",
                "structured",
                "-c",
                "sandbox_mode=read-only"
            ])
        );
    }

    #[test]
    fn direct_start_ignores_a_conflicting_real_herdr_workspace() {
        // Re-run a start fixture with a conflicting workspace in the real process environment.
        // The fake Herdr reports its own workspace, so a fixture that still inherited
        // HERDR_WORKSPACE_ID would refuse its own launch whenever the suite runs inside a pane.
        let output = std::process::Command::new(
            std::env::current_exe().expect("resolve current test executable"),
        )
        .arg("--exact")
        .arg("subagents::tests::direct_start_preserves_unstructured_raw_policy_and_nonoverriding_codex_config")
        .arg("--test-threads=1")
        .env("HERDR_WORKSPACE_ID", "conflicting-workspace")
        .output()
        .expect("run start fixture under a conflicting workspace");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("1 passed"), "{stdout}");
    }

    #[test]
    fn direct_start_prefers_named_workspace_then_inherited_workspace() {
        let named = Fixture::new();
        named
            .manager()
            .with_inherited_workspace(Some("elsewhere"))
            .start(
                "worker",
                &named.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    ..StartOptions::default()
                },
            )
            .unwrap();

        let inherited = Fixture::new();
        let error = inherited
            .manager()
            .with_inherited_workspace(Some("elsewhere"))
            .start("worker", &inherited.root, StartOptions::default())
            .unwrap_err();
        assert!(error.to_string().contains("workspace identity changed"));
        let record: Value = serde_json::from_slice(
            &fs::read(inherited.root.join("registry/worker/agent.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["workspace_id"], "elsewhere");
    }

    #[test]
    fn agent_record_v3_has_one_tagged_launch_and_goal_authority() {
        let fixture = Fixture::new();
        let started = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    model: Some("model-a".to_owned()),
                    harness_args: vec!["--literal".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let path = fixture.root.join("registry/worker/agent.json");
        let stored = agent::read_private_json(&path).unwrap();
        assert_eq!(stored["schema"], SESSION_STORAGE_SCHEMA);
        assert_eq!(stored["launch"]["schema"], LAUNCH_SPEC_SCHEMA);
        assert_eq!(stored["goal"]["schema"], GOAL_STATE_SCHEMA);
        assert_eq!(
            stored["native_session"],
            json!({
                "schema": NATIVE_SESSION_SCHEMA,
                "agent": "codex",
                "value": "thread",
                "source": "observed",
            })
        );
        assert_eq!(
            stored["launch"]["argv"],
            json!([
                "codex",
                "--no-alt-screen",
                "--model",
                "model-a",
                "--literal"
            ])
        );
        for duplicate in [
            "harness",
            "cwd",
            "adapter",
            "mode",
            "backend",
            "model",
            "resume",
            "arguments",
            "launch_argv",
            "launch_executable",
            "runtime_home",
            "runtime_ownership",
            "runner_pid",
            "runner_started_at",
            "session_agent",
            "session_value",
            "goal_delivery",
            "goal_session_id",
            "goal_command",
            "goal_messages",
            "goal_message_id",
        ] {
            assert!(
                stored.get(duplicate).is_none(),
                "duplicate field {duplicate}"
            );
        }
        assert_eq!(
            fixture.manager().get("worker").unwrap()["arguments"],
            started["arguments"]
        );
    }

    #[test]
    fn agent_record_v3_reads_profiled_claude_launch_without_flat_harness_field() {
        let fixture = Fixture::new();
        fixture.start(None);
        let path = fixture.root.join("registry/worker/agent.json");
        let mut stored = agent::read_private_json(&path).unwrap();
        stored["launch"]["harness"] = json!("claude");
        stored["launch"]["argv"] = json!(["claude", "--model", "opus"]);
        stored["launch"]["model"] = json!("opus");
        stored["launch"]["profile"] = json!("claude-opus-55");
        stored["native_session"]["agent"] = json!("claude");
        agent::atomic_json(&path, &stored).unwrap();
        let stored = agent::read_private_json(&path).unwrap();
        assert!(stored.get("harness").is_none());
        assert_eq!(stored["launch"]["harness"], "claude");
        assert_eq!(stored["launch"]["profile"], "claude-opus-55");
        assert_eq!(
            stored["launch"]["argv"],
            json!(["claude", "--model", "opus"])
        );
        let loaded = fixture.manager().load("worker").unwrap();
        assert_eq!(loaded.launch.harness, "claude");
        assert_eq!(loaded.launch.profile.as_deref(), Some("claude-opus-55"));
    }

    #[test]
    fn agent_record_v3_rejects_duplicate_nested_launch_keys() {
        let fixture = Fixture::new();
        fixture.start(None);
        let path = fixture.root.join("registry/worker/agent.json");
        let encoded = fs::read_to_string(&path).unwrap();
        let needle = "\"harness\": \"codex\"";
        assert_eq!(encoded.matches(needle).count(), 1);
        fs::write(
            &path,
            encoded.replacen(needle, "\"harness\": \"codex\", \"harness\": \"muse\"", 1),
        )
        .unwrap();

        let error = fixture.manager().load("worker").unwrap_err();
        assert!(
            error.to_string().contains("duplicate JSON object key"),
            "{error}"
        );
    }

    #[test]
    fn agent_record_v3_writer_refuses_incomplete_native_session() {
        let fixture = Fixture::new();
        fixture.start(None);
        let mut record = fixture.manager().load("worker").unwrap();
        record.session_value = None;
        record.session_source = None;

        let error = record.storage_value().unwrap_err();
        assert!(
            error.to_string().contains("incomplete native session"),
            "{error}"
        );
    }

    #[test]
    fn agent_record_v3_refuses_native_session_for_another_harness() {
        let fixture = Fixture::new();
        fixture.start(None);
        let path = fixture.root.join("registry/worker/agent.json");
        let mut stored = agent::read_private_json(&path).unwrap();
        stored["native_session"]["agent"] = json!("claude");
        agent::atomic_json(&path, &stored).unwrap();

        let error = fixture.manager().load("worker").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("native session harness mismatch"),
            "{error}"
        );
    }

    fn downgrade_current_record_to_v2(mut stored: Value) -> Value {
        let object = stored
            .as_object_mut()
            .expect("stored agent record is an object");
        let goal = object
            .remove("goal")
            .and_then(|value| value.as_object().cloned())
            .expect("current goal state");
        let native = object.remove("native_session");
        object.insert("schema".to_owned(), json!(LEGACY_SESSION_STORAGE_SCHEMA));
        match native {
            Some(Value::Object(native)) => {
                object.insert(
                    "session_agent".to_owned(),
                    native.get("agent").cloned().unwrap_or(Value::Null),
                );
                object.insert(
                    "session_value".to_owned(),
                    native.get("value").cloned().unwrap_or(Value::Null),
                );
                object.insert(
                    "goal_session_id".to_owned(),
                    native.get("value").cloned().unwrap_or(Value::Null),
                );
            }
            Some(Value::Null) | None => {
                object.insert("session_agent".to_owned(), Value::Null);
                object.insert("session_value".to_owned(), Value::Null);
                object.insert("goal_session_id".to_owned(), Value::Null);
            }
            Some(_) => panic!("current native session is an object or null"),
        }
        object.insert(
            "goal".to_owned(),
            goal.get("objective").cloned().unwrap_or(Value::Null),
        );
        object.insert("goal_delivery".to_owned(), Value::Null);
        object.insert(
            "goal_command".to_owned(),
            goal.get("native_command").cloned().unwrap_or(Value::Null),
        );
        object.insert("goal_messages".to_owned(), json!({}));
        object.insert(
            "goal_message_id".to_owned(),
            goal.get("message_id").cloned().unwrap_or(Value::Null),
        );
        stored
    }

    #[test]
    fn agent_record_v3_migrates_v2_goal_and_native_session_without_duplicates() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let path = fixture.root.join("registry/worker/agent.json");
        agent::enqueue(
            &fixture.root.join("registry/worker/queue"),
            "ordinary message",
            Some("ordinary"),
        )
        .unwrap();
        let mut legacy = downgrade_current_record_to_v2(agent::read_private_json(&path).unwrap());
        legacy["goal"] = json!("legacy objective");
        legacy["goal_delivery"] = json!("delivered");
        legacy["goal_command"] = json!(["codex", "app-server", "proxy"]);
        agent::atomic_json(&path, &legacy).unwrap();

        assert_eq!(
            manager.load("worker").unwrap().goal.as_deref(),
            Some("legacy objective")
        );
        manager.pause("worker", true).unwrap();
        let migrated = agent::read_private_json(&path).unwrap();
        assert_eq!(migrated["schema"], SESSION_STORAGE_SCHEMA);
        assert_eq!(
            migrated["goal"],
            json!({
                "schema": GOAL_STATE_SCHEMA,
                "objective": "legacy objective",
                "message_id": null,
                "native_command": ["codex", "app-server", "proxy"],
            })
        );
        assert_eq!(migrated["native_session"]["value"], "thread");
        for duplicate in [
            "session_agent",
            "session_value",
            "goal_delivery",
            "goal_session_id",
            "goal_command",
            "goal_messages",
            "goal_message_id",
        ] {
            assert!(migrated.get(duplicate).is_none(), "duplicate {duplicate}");
        }
    }

    #[test]
    fn agent_record_v2_refuses_conflicting_native_session_authorities() {
        let fixture = Fixture::new();
        fixture.start(None);
        let path = fixture.root.join("registry/worker/agent.json");
        let mut legacy = downgrade_current_record_to_v2(agent::read_private_json(&path).unwrap());
        legacy["goal_session_id"] = json!("different-session");
        agent::atomic_json(&path, &legacy).unwrap();

        assert!(fixture
            .manager()
            .load("worker")
            .unwrap_err()
            .to_string()
            .contains("contradictory native session"));
    }

    #[test]
    fn agent_record_v3_migrates_legacy_goal_artifact_before_dropping_map() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let queue = fixture.root.join("registry/worker/queue");
        let identifier =
            agent::enqueue(&queue, "/goal legacy objective", Some("legacy-goal")).unwrap();
        let artifact_path = queue.join("inbox").join(format!("{identifier}.json"));
        let mut legacy_artifact = agent::read_private_json(&artifact_path).unwrap();
        legacy_artifact.as_object_mut().unwrap().remove("kind");
        agent::atomic_json(&artifact_path, &legacy_artifact).unwrap();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut legacy = downgrade_current_record_to_v2(agent::read_private_json(&path).unwrap());
        legacy["goal"] = json!("legacy objective");
        legacy["goal_message_id"] = json!(identifier);
        legacy["goal_messages"] = json!({identifier.clone(): "legacy objective"});
        agent::atomic_json(&path, &legacy).unwrap();

        manager.pause("worker", true).unwrap();

        let migrated = agent::read_private_json(&path).unwrap();
        assert_eq!(migrated["schema"], SESSION_STORAGE_SCHEMA);
        let artifact = agent::read_private_json(&artifact_path).unwrap();
        assert_eq!(artifact["kind"], "goal");
        assert!(migrated.get("goal_messages").is_none());
    }

    #[test]
    fn legacy_goal_pointer_without_duplicate_map_is_exactly_recovered() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let objective = "recover the retained timerfd worktree";
        let prompt = goal_prompt("muse", objective);
        let queue = fixture.root.join("registry/worker/queue");
        let identifier = agent::enqueue(&queue, &prompt, Some("legacy-goal")).unwrap();
        let artifact_path = queue.join("inbox").join(format!("{identifier}.json"));
        let mut artifact = agent::read_private_json(&artifact_path).unwrap();
        artifact.as_object_mut().unwrap().remove("kind");
        agent::atomic_json(&artifact_path, &artifact).unwrap();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut legacy = downgrade_current_record_to_v2(agent::read_private_json(&path).unwrap());
        legacy["goal"] = json!(objective);
        legacy["goal_delivery"] = json!("possibly_submitted");
        legacy["goal_message_id"] = json!(identifier);
        legacy["goal_messages"] = json!({});
        agent::atomic_json(&path, &legacy).unwrap();

        assert_eq!(
            manager.status("worker").unwrap()["goal_delivery"],
            "pending"
        );
        manager.pause("worker", true).unwrap();

        assert_eq!(
            agent::read_private_json(&path).unwrap()["schema"],
            SESSION_STORAGE_SCHEMA
        );
        assert_eq!(
            agent::read_private_json(&artifact_path).unwrap()["kind"],
            "goal"
        );
    }

    #[test]
    fn legacy_goal_pointer_without_map_refuses_wrong_id_or_text() {
        for mutation in ["id", "text", "map"] {
            let fixture = Fixture::new();
            let manager = fixture.manager();
            fixture.start(None);
            let objective = "recover the retained timerfd worktree";
            let queue = fixture.root.join("registry/worker/queue");
            let identifier = agent::enqueue(
                &queue,
                &goal_prompt("codex", objective),
                Some("legacy-goal"),
            )
            .unwrap();
            let artifact_path = queue.join("inbox").join(format!("{identifier}.json"));
            let mut artifact = agent::read_private_json(&artifact_path).unwrap();
            artifact.as_object_mut().unwrap().remove("kind");
            if mutation != "map" {
                artifact[mutation] = json!("wrong");
            }
            agent::atomic_json(&artifact_path, &artifact).unwrap();
            let path = fixture.root.join("registry/worker/agent.json");
            let mut legacy =
                downgrade_current_record_to_v2(agent::read_private_json(&path).unwrap());
            legacy["goal"] = json!(objective);
            legacy["goal_delivery"] = json!("possibly_submitted");
            legacy["goal_message_id"] = json!(identifier.clone());
            legacy["goal_messages"] = if mutation == "map" {
                json!({identifier.clone(): "wrong"})
            } else {
                json!({})
            };
            agent::atomic_json(&path, &legacy).unwrap();

            assert!(manager.status("worker").is_err(), "mutation {mutation}");
            assert!(
                manager.pause("worker", true).is_err(),
                "mutation {mutation}"
            );
        }
    }

    #[test]
    fn current_goal_pointer_never_accepts_an_untagged_artifact() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let queue = fixture.root.join("registry/worker/queue");
        let identifier = agent::enqueue(&queue, "/goal exact", Some("current-goal")).unwrap();
        let artifact_path = queue.join("inbox").join(format!("{identifier}.json"));
        let mut artifact = agent::read_private_json(&artifact_path).unwrap();
        artifact.as_object_mut().unwrap().remove("kind");
        agent::atomic_json(&artifact_path, &artifact).unwrap();
        let mut record = manager.load("worker").unwrap();
        record.goal = Some("exact".to_owned());
        record.goal_message_id = Some(identifier);
        assert!(manager.save(&record).is_err());
    }

    #[test]
    fn current_goal_pointer_without_artifact_or_transaction_is_rejected() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut stored = agent::read_private_json(&path).unwrap();
        stored["goal"] = json!({
            "schema": GOAL_STATE_SCHEMA,
            "objective": "missing objective",
            "message_id": "missing-goal",
            "native_command": null,
        });
        agent::atomic_json(&path, &stored).unwrap();

        let error = manager.status("worker").unwrap_err();
        assert!(error.to_string().contains("no durable queue artifact"));
    }

    #[test]
    fn goal_transaction_recovers_every_publication_prefix() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let transaction_path = fixture.root.join("registry/worker/goal-transaction.json");
        let queue = fixture.root.join("registry/worker/queue");
        let objective = "recover the retained timerfd worktree";
        let prompt = goal_prompt("muse", objective);

        let record = manager.load("worker").unwrap();
        manager
            .write_goal_transaction(&record, "goal-before-record", objective)
            .unwrap();
        manager.reconcile_goal_transaction(&record).unwrap();
        assert!(!transaction_path.exists());
        assert_eq!(
            agent::message_state(&queue, "goal-before-record").unwrap(),
            None
        );

        let mut record = manager.load("worker").unwrap();
        manager
            .write_goal_transaction(&record, "goal-after-record", objective)
            .unwrap();
        record.goal = Some(objective.to_owned());
        record.goal_message_id = Some("goal-after-record".to_owned());
        manager.save(&record).unwrap();
        assert_eq!(
            manager.status("worker").unwrap()["goal_delivery"],
            "pending"
        );
        manager
            .reconcile_goal_transaction(&manager.load("worker").unwrap())
            .unwrap();
        assert!(!transaction_path.exists());
        let artifact =
            agent::read_private_json(&queue.join("inbox/goal-after-record.json")).unwrap();
        assert_eq!(artifact["kind"], "goal");
        assert_eq!(artifact["text"], prompt);

        let mut record = manager.load("worker").unwrap();
        manager
            .write_goal_transaction(&record, "goal-after-artifact", objective)
            .unwrap();
        record.goal = Some(objective.to_owned());
        record.goal_message_id = Some("goal-after-artifact".to_owned());
        manager.save(&record).unwrap();
        agent::enqueue_goal(&queue, &prompt, "goal-after-artifact").unwrap();
        manager
            .reconcile_goal_transaction(&manager.load("worker").unwrap())
            .unwrap();
        assert!(!transaction_path.exists());
        assert_eq!(
            agent::message_state(&queue, "goal-after-artifact").unwrap(),
            Some(agent::QueueMessageState::Pending)
        );
    }

    #[test]
    fn goal_delivery_is_derived_from_every_exact_queue_transition() {
        for (folder, expected) in [
            ("inbox", "pending"),
            ("inflight", "possibly_submitted"),
            ("failed", "possibly_submitted"),
            ("processed", "delivered"),
        ] {
            let fixture = Fixture::new();
            fixture.start(None);
            let manager = fixture.manager();
            let queue = fixture.root.join("registry/worker/queue");
            let identifier = "goal-transition";
            agent::enqueue_goal(&queue, "/goal finish exact work", identifier).unwrap();
            if folder != "inbox" {
                fs::rename(
                    queue.join("inbox").join(format!("{identifier}.json")),
                    queue.join(folder).join(format!("{identifier}.json")),
                )
                .unwrap();
            }
            let mut record = manager.load("worker").unwrap();
            record.goal = Some("finish exact work".to_owned());
            record.goal_message_id = Some(identifier.to_owned());
            manager.save(&record).unwrap();
            assert_eq!(manager.status("worker").unwrap()["goal_delivery"], expected);
        }
    }

    #[test]
    fn send_migrates_legacy_goal_authority_before_queue_delivery() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let queue = fixture.root.join("registry/worker/queue");
        let identifier =
            agent::enqueue(&queue, "/goal legacy objective", Some("legacy-goal")).unwrap();
        let artifact_path = queue.join("inbox").join(format!("{identifier}.json"));
        let mut legacy_artifact = agent::read_private_json(&artifact_path).unwrap();
        legacy_artifact.as_object_mut().unwrap().remove("kind");
        agent::atomic_json(&artifact_path, &legacy_artifact).unwrap();
        let record_path = fixture.root.join("registry/worker/agent.json");
        let mut legacy =
            downgrade_current_record_to_v2(agent::read_private_json(&record_path).unwrap());
        legacy["goal"] = json!("legacy objective");
        legacy["goal_message_id"] = json!(identifier.clone());
        legacy["goal_messages"] = json!({identifier.clone(): "legacy objective"});
        agent::atomic_json(&record_path, &legacy).unwrap();

        manager
            .send("worker", "ordinary message", DrainOptions::default())
            .unwrap();

        let migrated = agent::read_private_json(&record_path).unwrap();
        assert_eq!(migrated["schema"], SESSION_STORAGE_SCHEMA);
        assert!(migrated.get("goal_messages").is_none());
        let processed = queue.join("processed").join(format!("{identifier}.json"));
        assert_eq!(
            agent::read_private_json(&processed).unwrap()["kind"],
            "goal"
        );
        let runs = fixture.client.runs.lock().unwrap().clone();
        assert_eq!(runs.len(), 2);
        assert!(runs.contains(&"ordinary message".to_owned()));
        assert!(runs.contains(&"/goal legacy objective".to_owned()));
    }

    #[test]
    fn agent_record_v3_refuses_to_discard_missing_legacy_goal_artifact() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let path = fixture.root.join("registry/worker/agent.json");
        agent::enqueue(
            &fixture.root.join("registry/worker/queue"),
            "ordinary message",
            Some("ordinary"),
        )
        .unwrap();
        let mut legacy = downgrade_current_record_to_v2(agent::read_private_json(&path).unwrap());
        legacy["goal"] = json!("missing objective");
        legacy["goal_message_id"] = json!("missing-goal");
        legacy["goal_messages"] = json!({"missing-goal": "missing objective"});
        agent::atomic_json(&path, &legacy).unwrap();
        let encoded = fs::read(&path).unwrap();

        let error = manager.pause("worker", true).unwrap_err();
        assert!(error.to_string().contains("refusing to discard"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), encoded);
    }

    #[test]
    fn goal_delivery_requires_the_exact_tagged_queue_artifact() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let queue = fixture.root.join("registry/worker/queue");
        let identifier = agent::enqueue(&queue, "/goal finish task", Some("goal-1")).unwrap();
        let artifact_path = queue.join("inbox").join(format!("{identifier}.json"));
        let mut artifact = agent::read_private_json(&artifact_path).unwrap();
        artifact["kind"] = json!("goal");
        agent::atomic_json(&artifact_path, &artifact).unwrap();
        let mut record = manager.load("worker").unwrap();
        record.goal = Some("finish task".to_owned());
        record.goal_message_id = Some(identifier.clone());
        manager.save(&record).unwrap();
        assert_eq!(
            manager.goal_delivery(&record).unwrap().as_deref(),
            Some("pending")
        );

        artifact["kind"] = json!("message");
        agent::atomic_json(&artifact_path, &artifact).unwrap();
        let error = manager.goal_delivery(&record).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("disagrees with its session record"),
            "{error}"
        );

        artifact["kind"] = json!("goal");
        artifact["text"] = json!("/goal forged task");
        agent::atomic_json(&artifact_path, &artifact).unwrap();
        let error = manager.goal_delivery(&record).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("disagrees with its session record"),
            "{error}"
        );
    }

    #[test]
    fn agent_record_v3_migrates_legacy_launch_spec_at_write_boundary() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut stored = agent::read_private_json(&path).unwrap();
        stored["launch"]["schema"] = json!(LEGACY_LAUNCH_SPEC_SCHEMA);
        stored["launch"]
            .as_object_mut()
            .unwrap()
            .remove("permission_mode");
        agent::atomic_json(&path, &stored).unwrap();

        assert_eq!(manager.load("worker").unwrap().launch.permission_mode, None);
        manager.pause("worker", true).unwrap();
        let migrated = agent::read_private_json(&path).unwrap();
        assert_eq!(migrated["launch"]["schema"], LAUNCH_SPEC_SCHEMA);
        assert!(migrated["launch"]["permission_mode"].is_null());
    }

    #[test]
    fn agent_record_v2_extensions_cannot_shadow_launch_authority() {
        for collision in [
            "launch_argv",
            "launch_profile",
            "argv",
            "profile",
            "adapter",
            "arguments",
            "runner_pid",
            "runner_started_at",
            "permission_mode",
            "launch_permission_mode",
            "launch",
            "native_session",
            "extensions",
            "goal_delivery",
            "goal_messages",
            "goal_session_id",
        ] {
            let fixture = Fixture::new();
            fixture.start(None);
            let path = fixture.root.join("registry/worker/agent.json");
            let mut document = agent::read_private_json(&path).unwrap();
            document["extensions"][collision] = match collision {
                "runner_pid" => json!(123),
                "runner_started_at" => json!("456"),
                "launch_profile"
                | "profile"
                | "adapter"
                | "permission_mode"
                | "launch_permission_mode"
                | "goal_delivery"
                | "goal_session_id" => json!("replacement"),
                "launch" | "native_session" | "extensions" | "goal_messages" => json!({}),
                _ => json!(["replacement"]),
            };
            agent::atomic_json(&path, &document).unwrap();
            assert!(fixture
                .manager()
                .load("worker")
                .unwrap_err()
                .to_string()
                .contains("invalid agent record extension"));
        }
    }

    #[test]
    fn agent_record_v1_migrates_once_and_refuses_conflicting_argument_authorities() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut legacy = manager.load("worker").unwrap().public_value();
        legacy["launch_argv"] = json!([]);
        legacy["arguments"] = json!(["--no-alt-screen", "--literal"]);
        agent::atomic_json(&path, &legacy).unwrap();
        assert_eq!(
            manager.load("worker").unwrap().launch.argv,
            ["codex", "--no-alt-screen", "--literal"]
        );
        manager.pause("worker", true).unwrap();
        let migrated = agent::read_private_json(&path).unwrap();
        assert_eq!(migrated["schema"], SESSION_STORAGE_SCHEMA);
        assert_eq!(
            migrated["launch"]["argv"],
            json!(["codex", "--no-alt-screen", "--literal"])
        );

        let mut conflicting = manager.load("worker").unwrap().public_value();
        conflicting["arguments"] = json!(["--different"]);
        agent::atomic_json(&path, &conflicting).unwrap();
        assert!(manager
            .load("worker")
            .unwrap_err()
            .to_string()
            .contains("contradictory launch arguments"));
    }

    #[test]
    fn agent_record_v1_rejects_reserved_current_schema_fields() {
        for field in ["launch", "native_session", "extensions"] {
            let fixture = Fixture::new();
            fixture.start(None);
            let path = fixture.root.join("registry/worker/agent.json");
            let mut legacy = fixture.manager().load("worker").unwrap().public_value();
            legacy[field] = json!({});
            agent::atomic_json(&path, &legacy).unwrap();
            let error = fixture.manager().load("worker").unwrap_err();
            assert!(
                error.to_string().contains("reserved current-schema field"),
                "{field}: {error}"
            );
        }
    }

    #[test]
    fn agent_record_v2_refuses_crossed_launch_dimensions() {
        for (field, value) in [
            ("adapter", "turn-runner"),
            ("mode", "headless"),
            ("backend", "tmux"),
            ("runtime_ownership", "foreign"),
        ] {
            let fixture = Fixture::new();
            fixture.start(None);
            let path = fixture.root.join("registry/worker/agent.json");
            let mut document = agent::read_private_json(&path).unwrap();
            document["launch"][field] = json!(value);
            agent::atomic_json(&path, &document).unwrap();
            assert!(fixture
                .manager()
                .load("worker")
                .unwrap_err()
                .to_string()
                .contains("inconsistent"));
        }
    }

    #[test]
    fn direct_start_rejects_quoted_config_and_opaque_profile_conflicts_before_allocation() {
        let fixture = Fixture::new();
        let model_error = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    model: Some("structured".to_owned()),
                    harness_args: vec!["--config=\"model\"=\"raw\"".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(model_error.to_string().contains("model field"));
        let effort_error = fixture
            .manager()
            .start_with_reasoning_effort(
                "worker",
                &fixture.root,
                "ultra",
                StartOptions {
                    harness_args: vec!["-c\"model_reasoning_effort\"=\"low\"".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(effort_error.to_string().contains("reasoning effort"));
        let profile_model_error = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    model: Some("structured".to_owned()),
                    harness_args: vec!["--profile".to_owned(), "attacker".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(profile_model_error
            .to_string()
            .contains("raw Codex profile"));
        let profile_effort_error = fixture
            .manager()
            .start_with_reasoning_effort(
                "worker",
                &fixture.root,
                "ultra",
                StartOptions {
                    harness_args: vec!["-pattacker".to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(profile_effort_error
            .to_string()
            .contains("raw Codex profile"));
        for arguments in [
            vec!["-c".to_owned(), "profile=attacker".to_owned()],
            vec!["--config=profile=attacker".to_owned()],
            vec!["-c\"profile\"=\"attacker\"".to_owned()],
        ] {
            let error = fixture
                .manager()
                .start(
                    "worker",
                    &fixture.root,
                    StartOptions {
                        model: Some("structured".to_owned()),
                        harness_args: arguments,
                        ..StartOptions::default()
                    },
                )
                .unwrap_err();
            assert!(error.to_string().contains("raw Codex profile"));
        }
        assert!(!fixture.root.join("registry").exists());
        assert!(!fixture.client.started.load(Ordering::Relaxed));
        assert!(fixture.client.environments.lock().unwrap().is_empty());
    }

    #[test]
    fn adoption_rejects_muse_before_registry_or_live_pane_access() {
        let fixture = Fixture::new();
        fixture.prepare_foreign();
        let mut options = fixture.adopt_options();
        options.harness = "muse".to_owned();
        let error = fixture.manager().adopt("foreign", options).unwrap_err();
        assert!(error.to_string().contains("adopting Muse is unsupported"));
        assert!(!fixture.root.join("registry").exists());
        assert_eq!(fixture.client.panes_calls.load(Ordering::Relaxed), 0);
        assert_eq!(fixture.client.pane_info_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn failed_launch_status_does_not_retain_environment_values() {
        let fixture = Fixture::new();
        let secret = "ACCESS_TOKEN=literal-sensitive-value";
        fixture.client.fail_panes.store(true, Ordering::Relaxed);
        let error = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    environment: vec![secret.to_owned()],
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("pane query failed"));
        let record = fixture.manager().load("worker").unwrap();
        assert_eq!(record.lifecycle, "launch_failed");
        assert_eq!(
            record.error.as_deref(),
            Some("launch failed with caller-supplied environment; details omitted from status")
        );
        assert!(!record.public_value().to_string().contains(secret));
    }

    #[test]
    fn start_session_change_leaves_launch_failed_record_stoppable() {
        let fixture = Fixture::new();
        fixture
            .client
            .change_owned_session_after_save
            .store(true, Ordering::Relaxed);
        let error = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("session"), "{error}");
        let failed = fixture.manager().load("worker").unwrap();
        assert_eq!(failed.lifecycle, "launch_failed");
        assert!(failed.session_agent.is_none() && failed.session_value.is_none());
        assert_eq!(
            fixture.manager().stop("worker").unwrap()["pane_closed"],
            true
        );
    }

    #[test]
    fn adopted_agent_keeps_native_identity_and_uses_the_durable_named_interface() {
        let fixture = Fixture::new();
        let adopted = fixture.adopt();
        let manager = fixture.manager();
        assert_eq!(adopted["adapter"], "herdr-foreign");
        assert_eq!(adopted["pane_id"], "owned");
        assert_eq!(adopted["session_agent"], "codex");
        assert_eq!(adopted["session_value"], "thread");
        assert_eq!(
            adopted["foreign_shell_identity"],
            json!(Fake::foreign_shell_identity())
        );
        assert_eq!(
            adopted["capabilities"],
            json!([
                "send",
                "status",
                "read",
                "wait",
                "stop",
                "attach",
                "pause",
                "resume",
                "terminal-snapshot",
                "drain",
                "goal",
                "bind-session",
                "relocate"
            ])
        );
        assert_eq!(manager.list().unwrap().len(), 1);
        manager
            .send("foreign", "follow up", DrainOptions::default())
            .unwrap();
        assert_eq!(manager.read("foreign", 10).unwrap(), "visible output");
        assert_eq!(
            manager.wait("foreign", Duration::from_secs(0)).unwrap()["agent_status"],
            "idle"
        );
        assert_eq!(
            manager.bind_session("foreign", "thread", None).unwrap()["source"],
            "explicit"
        );
        let command = ["/bin/false".to_owned()];
        let goal = manager
            .goal(
                "foreign",
                Some("finish adopted work"),
                DrainOptions::default(),
                Some(&command),
            )
            .unwrap();
        assert_eq!(goal["delivery"], "delivered");
        assert_eq!(
            *fixture.client.runs.lock().unwrap(),
            ["follow up", "/goal finish adopted work"]
        );
        let binding =
            agent::read_private_json(&fixture.root.join("registry/foreign/queue/target.json"))
                .unwrap();
        assert_eq!(binding["kind"], "session");
        assert_eq!(binding["agent"], "codex");
        assert_eq!(binding["value"], "thread");
    }

    #[test]
    fn every_adopted_operation_revalidates_the_shell_generation() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture
            .client
            .foreign_shell_identity
            .lock()
            .unwrap()
            .starttime_ticks += 1;
        let manager = fixture.manager();

        let status = manager.status("foreign").unwrap();
        assert_eq!(status["agent_status"], "unknown");
        assert!(status["probe_error"]
            .as_str()
            .unwrap()
            .contains("shell generation changed"));
        assert!(manager
            .send("foreign", "message", DrainOptions::default())
            .is_err());
        assert!(manager.read("foreign", 10).is_err());
        assert!(manager.wait("foreign", Duration::ZERO).is_err());
        assert!(manager.pause("foreign", true).is_err());
        assert!(manager.attach("foreign").is_err());
        assert!(manager.bind_session("foreign", "thread", None).is_err());
        assert!(fixture.client.runs.lock().unwrap().is_empty());
    }

    #[test]
    fn stopping_an_adopted_agent_only_unregisters_and_archives_control_state() {
        let fixture = Fixture::new();
        let adopted = fixture.adopt();
        let manager = fixture.manager();
        manager
            .send("foreign", "retained request", DrainOptions::default())
            .unwrap();
        let presentation = Fake::pane("owned");

        let stopped = manager.stop("foreign").unwrap();

        assert_eq!(stopped["runtime_preserved"], true);
        assert_eq!(stopped["pane_closed"], false);
        assert_eq!(stopped["tab_closed"], false);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(*fixture.client.panes.lock().unwrap(), [presentation]);
        let archive = PathBuf::from(stopped["archive"].as_str().unwrap());
        let saved = agent::read_private_json(&archive.join("agent.json")).unwrap();
        assert_eq!(saved["launch"]["adapter"], "herdr-foreign");
        assert_eq!(saved["token"], adopted["token"]);
        assert!(archive.join("output.json").is_file());
        assert_eq!(
            fs::read_dir(archive.join("queue/processed"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn adoption_archives_a_failed_generation_if_the_shell_changes_after_save() {
        let fixture = Fixture::new();
        fixture.prepare_foreign();
        fixture
            .client
            .change_foreign_shell_after_save
            .store(true, Ordering::Relaxed);

        let error = fixture
            .manager()
            .adopt("foreign", fixture.adopt_options())
            .unwrap_err();

        assert!(error.to_string().contains("was not registered"));
        assert!(!fixture.root.join("registry/foreign").exists());
        let archives = fs::read_dir(fixture.root.join("registry/archive"))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(archives.len(), 1);
        let saved = agent::read_private_json(&archives[0].path().join("agent.json")).unwrap();
        assert_eq!(saved["lifecycle"], "adopt_failed");
        assert!(
            saved["error"]
                .as_str()
                .unwrap()
                .contains("shell generation changed"),
            "{}",
            saved["error"]
        );
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn stopping_a_confirmed_dead_adopted_agent_only_archives_control_state() {
        let fixture = Fixture::new();
        let adopted = fixture.adopt();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let presentation = Fake::pane("owned");

        let stopped = fixture.manager().stop("foreign").unwrap();

        assert_eq!(stopped["runtime_preserved"], true);
        assert_eq!(stopped["pane_closed"], false);
        assert_eq!(stopped["tab_closed"], false);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(*fixture.client.panes.lock().unwrap(), [presentation]);
        let archive = PathBuf::from(stopped["archive"].as_str().unwrap());
        let saved = agent::read_private_json(&archive.join("agent.json")).unwrap();
        assert_eq!(saved["token"], adopted["token"]);
        assert!(archive.join("output.json").is_file());
    }

    #[test]
    fn stopping_an_absent_adopted_agent_refuses_a_non_idle_pane() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture
            .client
            .custom_at_idle_shell
            .store(false, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error.to_string().contains("identity-bound idle shell"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_an_absent_adopted_agent_refuses_native_session_residue() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture.client.started.store(false, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error
            .to_string()
            .contains("absent agent has native session identity"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_an_absent_adopted_agent_refuses_replaced_shell_generations() {
        let original = Fake::foreign_shell_identity();
        let mut replacements = Vec::new();
        let mut new_pane_shell = original.clone();
        new_pane_shell.pid += 1;
        replacements.push(new_pane_shell);
        let mut reused_pid = original.clone();
        reused_pid.starttime_ticks += 1;
        replacements.push(reused_pid);
        let mut replaced_image = original;
        replaced_image.executable_inode += 1;
        replacements.push(replaced_image);

        for replacement in replacements {
            let fixture = Fixture::new();
            fixture.adopt();
            fixture.client.started.store(false, Ordering::Relaxed);
            fixture
                .client
                .report_session
                .store(false, Ordering::Relaxed);
            *fixture.client.foreign_shell_identity.lock().unwrap() = replacement;

            let error = fixture.manager().stop("foreign").unwrap_err();

            assert!(error
                .to_string()
                .contains("recorded pane shell generation changed"));
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert_eq!(
                fixture.manager().load("foreign").unwrap().lifecycle,
                "running"
            );
        }
    }

    #[test]
    fn stopping_an_absent_legacy_adopted_agent_refuses_missing_shell_identity() {
        let fixture = Fixture::new();
        fixture.adopt();
        let path = fixture.root.join("registry/foreign/agent.json");
        let mut document = fixture.manager().load("foreign").unwrap().public_value();
        document
            .as_object_mut()
            .unwrap()
            .remove("foreign_shell_identity");
        agent::atomic_json(&path, &document).unwrap();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error
            .to_string()
            .contains("legacy record has no identity-bound"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn explicit_legacy_recovery_preserves_record_queue_and_foreign_runtime() {
        let fixture = Fixture::new();
        let adopted = fixture.adopt();
        fixture
            .manager()
            .send("foreign", "retained request", DrainOptions::default())
            .unwrap();
        let record_path = fixture.root.join("registry/foreign/agent.json");
        let mut document = fixture.manager().load("foreign").unwrap().public_value();
        document
            .as_object_mut()
            .unwrap()
            .remove("foreign_shell_identity");
        agent::atomic_json(&record_path, &document).unwrap();
        let raw = fs::read(&record_path).unwrap();
        let digest = format!("{:x}", Sha256::digest(&raw));
        let queue_before = fs::read(
            fixture
                .root
                .join("registry/foreign/queue/processed")
                .read_dir()
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let presentation = Fake::pane("owned");

        let stopped = fixture
            .manager()
            .stop_with_options(
                "foreign",
                StopOptions {
                    expected_token: Some(adopted["token"].as_str().unwrap().to_owned()),
                    recover_legacy_adoption: true,
                    expected_record_sha256: Some(digest.clone()),
                },
            )
            .unwrap();

        let archive = PathBuf::from(stopped["archive"].as_str().unwrap());
        assert_eq!(stopped["recovered_legacy_adoption"], true);
        assert_eq!(stopped["runtime_preserved"], true);
        assert_eq!(stopped["record_sha256"], digest);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(*fixture.client.panes.lock().unwrap(), [presentation]);
        assert_eq!(fs::read(archive.join("agent.json")).unwrap(), raw);
        let archived_queue = fs::read(
            archive
                .join("queue/processed")
                .read_dir()
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        assert_eq!(archived_queue, queue_before);
        assert!(archive.join("output.json").is_file());
    }

    #[test]
    fn explicit_legacy_recovery_requires_exact_options_and_absent_raw_identity_key() {
        for case in [
            "missing-token",
            "missing-hash",
            "wrong-token",
            "wrong-hash",
            "null",
        ] {
            let fixture = Fixture::new();
            let (token, mut digest, _) = fixture.make_legacy_dead();
            if case == "null" {
                let path = fixture.root.join("registry/foreign/agent.json");
                let mut document = agent::read_private_json(&path).unwrap();
                document["foreign_shell_identity"] = Value::Null;
                agent::atomic_json(&path, &document).unwrap();
                digest = format!("{:x}", Sha256::digest(fs::read(path).unwrap()));
            }
            let options = StopOptions {
                expected_token: (case != "missing-token").then(|| {
                    if case == "wrong-token" {
                        "wrong-generation".to_owned()
                    } else {
                        token.clone()
                    }
                }),
                recover_legacy_adoption: true,
                expected_record_sha256: (case != "missing-hash").then(|| {
                    if case == "wrong-hash" {
                        "0".repeat(64)
                    } else {
                        digest.clone()
                    }
                }),
            };
            assert!(fixture
                .manager()
                .stop_with_options("foreign", options)
                .is_err());
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.root.join("registry/foreign").is_dir());
        }
    }

    #[test]
    fn explicit_legacy_recovery_refuses_all_shell_generation_changes_during_capture() {
        let original = Fake::foreign_shell_identity();
        let mut replacements = Vec::new();
        let mut value = original.clone();
        value.pid += 1;
        replacements.push(PaneShellProof {
            identity: value,
            executable_path: PathBuf::from("/bin/bash"),
        });
        let mut value = original.clone();
        value.starttime_ticks += 1;
        replacements.push(PaneShellProof {
            identity: value,
            executable_path: PathBuf::from("/bin/bash"),
        });
        let mut value = original.clone();
        value.boot_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".to_owned();
        replacements.push(PaneShellProof {
            identity: value,
            executable_path: PathBuf::from("/bin/bash"),
        });
        let mut value = original.clone();
        value.executable_device += 1;
        replacements.push(PaneShellProof {
            identity: value,
            executable_path: PathBuf::from("/bin/bash"),
        });
        let mut value = original.clone();
        value.executable_inode += 1;
        replacements.push(PaneShellProof {
            identity: value,
            executable_path: PathBuf::from("/bin/bash"),
        });
        replacements.push(PaneShellProof {
            identity: original,
            executable_path: PathBuf::from("/bin/dash"),
        });

        for replacement in replacements {
            let fixture = Fixture::new();
            let (token, digest, _) = fixture.make_legacy_dead();
            *fixture.client.replacement_shell_on_read.lock().unwrap() = Some(replacement);
            let error = fixture
                .manager()
                .stop_with_options(
                    "foreign",
                    StopOptions {
                        expected_token: Some(token),
                        recover_legacy_adoption: true,
                        expected_record_sha256: Some(digest),
                    },
                )
                .unwrap_err();
            assert!(error.to_string().contains("runtime identity changed"));
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.root.join("registry/foreign").is_dir());
        }
    }

    #[test]
    fn explicit_legacy_recovery_refuses_live_busy_session_moved_or_ambiguous_panes() {
        for case in ["live", "busy", "session", "moved", "sibling", "duplicate"] {
            let fixture = Fixture::new();
            let (token, digest, _) = fixture.make_legacy_dead();
            match case {
                "live" => fixture.client.started.store(true, Ordering::Relaxed),
                "busy" => fixture
                    .client
                    .custom_at_idle_shell
                    .store(false, Ordering::Relaxed),
                "session" => fixture.client.report_session.store(true, Ordering::Relaxed),
                "moved" => fixture.client.panes.lock().unwrap()[0].tab_id = "moved".to_owned(),
                "sibling" => fixture
                    .client
                    .panes
                    .lock()
                    .unwrap()
                    .push(Fake::pane("sibling")),
                "duplicate" => {
                    let mut duplicate = Fake::pane("owned");
                    duplicate.tab_id = "duplicate".to_owned();
                    fixture.client.panes.lock().unwrap().push(duplicate);
                }
                _ => unreachable!(),
            }
            assert!(fixture
                .manager()
                .stop_with_options(
                    "foreign",
                    StopOptions {
                        expected_token: Some(token),
                        recover_legacy_adoption: true,
                        expected_record_sha256: Some(digest),
                    },
                )
                .is_err());
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.root.join("registry/foreign").is_dir());
        }
    }

    #[test]
    fn explicit_legacy_recovery_refuses_record_change_or_capture_failure() {
        for case in ["record", "directory", "capture", "procfs"] {
            let fixture = Fixture::new();
            let (token, digest, _) = fixture.make_legacy_dead();
            if case == "record" {
                fixture
                    .client
                    .change_record_token_on_read
                    .store(true, Ordering::Relaxed);
            } else if case == "directory" {
                fixture
                    .client
                    .replace_record_directory_on_read
                    .store(true, Ordering::Relaxed);
            } else if case == "capture" {
                fixture.client.fail_read.store(true, Ordering::Relaxed);
            } else {
                fixture
                    .client
                    .fail_shell_proof
                    .store(true, Ordering::Relaxed);
            }
            let error = fixture
                .manager()
                .stop_with_options(
                    "foreign",
                    StopOptions {
                        expected_token: Some(token),
                        recover_legacy_adoption: true,
                        expected_record_sha256: Some(digest),
                    },
                )
                .unwrap_err();
            if matches!(case, "capture" | "procfs") {
                assert_eq!(error.exit_code(), 75);
            }
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.root.join("registry/foreign").is_dir());
        }
    }

    #[test]
    fn explicit_legacy_recovery_binds_absent_key_and_parsed_record_to_same_bytes() {
        let fixture = Fixture::new();
        let (token, digest, _) = fixture.make_legacy_dead();
        let output = fixture.root.join("registry/foreign/output.json");
        let prior = b"{\"prior\":\"evidence\"}\n";
        fs::write(&output, prior).unwrap();
        fs::set_permissions(&output, fs::Permissions::from_mode(0o600)).unwrap();
        fixture
            .client
            .add_null_shell_identity_on_read
            .store(true, Ordering::Relaxed);

        let error = fixture
            .manager()
            .stop_with_options(
                "foreign",
                StopOptions {
                    expected_token: Some(token),
                    recover_legacy_adoption: true,
                    expected_record_sha256: Some(digest),
                },
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("requires foreign_shell_identity to be absent"));
        assert_eq!(fs::read(output).unwrap(), prior);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn explicit_legacy_recovery_rechecks_record_after_output_preparation() {
        let fixture = Fixture::new();
        let (token, digest, _) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let path = fixture.root.join("registry/foreign/agent.json");
        let options = StopOptions {
            expected_token: Some(token),
            recover_legacy_adoption: true,
            expected_record_sha256: Some(digest),
        };

        let error = manager
            .recover_legacy_adoption_locked_with(&record, &pinned, &options, || {
                let mut document = agent::read_private_json(&path).unwrap();
                document["foreign_shell_identity"] = Value::Null;
                agent::atomic_json(&path, &document).unwrap();
            })
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("requires foreign_shell_identity to be absent"));
        assert!(fixture.root.join("registry/foreign").is_dir());
        assert!(!fixture.root.join("registry/foreign/output.json").exists());
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn explicit_legacy_recovery_refuses_oversize_serialized_output() {
        let fixture = Fixture::new();
        let (token, digest, raw) = fixture.make_legacy_dead();
        fixture.client.oversized_read.store(true, Ordering::Relaxed);

        let error = fixture
            .manager()
            .stop_with_options(
                "foreign",
                StopOptions {
                    expected_token: Some(token.clone()),
                    recover_legacy_adoption: true,
                    expected_record_sha256: Some(digest),
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("output.json larger"));
        assert_eq!(
            fs::read(fixture.root.join("registry/foreign/agent.json")).unwrap(),
            raw
        );
        assert!(!fixture.root.join("registry/foreign/output.json").exists());
        assert!(!fixture
            .root
            .join(format!("registry/archive/foreign-{token}"))
            .exists());
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn explicit_legacy_recovery_uses_utf8_snapshot_budget() {
        let fixture = Fixture::new();
        let (token, digest, _) = fixture.make_legacy_dead();
        fixture.client.non_ascii_read.store(true, Ordering::Relaxed);
        let result = fixture
            .manager()
            .stop_with_options(
                "foreign",
                StopOptions {
                    expected_token: Some(token),
                    recover_legacy_adoption: true,
                    expected_record_sha256: Some(digest),
                },
            )
            .unwrap();
        let output = PathBuf::from(result["archive"].as_str().unwrap()).join("output.json");
        let bytes = fs::read(&output).unwrap();

        assert!(bytes.len() < MAX_SNAPSHOT_BYTES);
        assert_eq!(
            agent::read_private_json(&output).unwrap()["text"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            3 << 20
        );
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn directory_pin_refuses_validate_to_open_replacement() {
        for target_kind in ["agent", "parent"] {
            for replacement in ["private", "public", "missing"] {
                let fixture = Fixture::new();
                fixture.make_legacy_dead();
                let manager = fixture.manager();
                let target = if target_kind == "agent" {
                    fixture.root.join("registry/foreign")
                } else {
                    let path = fixture.root.join("registry/archive");
                    fs::create_dir(&path).unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                    path
                };
                let displaced = target.with_file_name(format!(
                    ".{}-original",
                    target.file_name().unwrap().to_string_lossy()
                ));
                let swap = || {
                    fs::rename(&target, &displaced).unwrap();
                    if replacement != "missing" {
                        fs::create_dir(&target).unwrap();
                        let mode = if replacement == "private" {
                            0o700
                        } else {
                            0o755
                        };
                        fs::set_permissions(&target, fs::Permissions::from_mode(mode)).unwrap();
                    }
                };

                let result = if target_kind == "agent" {
                    manager
                        .pinned_agent_directory_with("foreign", swap)
                        .map(|_| ())
                } else {
                    ManagedAgents::<Fake>::pinned_parent_directory_with(
                        &target,
                        "agent archive",
                        swap,
                    )
                    .map(|_| ())
                };

                assert!(result.is_err(), "{target_kind}/{replacement} was pinned");
                assert!(displaced.is_dir());
                assert_eq!(target.exists(), replacement != "missing");
            }
        }
    }

    #[test]
    fn pinned_directory_recheck_refuses_postopen_permission_change() {
        let fixture = Fixture::new();
        fixture.make_legacy_dead();
        let manager = fixture.manager();
        let agent_path = fixture.root.join("registry/foreign");
        let agent_pinned = manager.pinned_agent_directory("foreign").unwrap();
        fs::set_permissions(&agent_path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ManagedAgents::<Fake>::verify_pinned_agent_directory(&agent_pinned).is_err());

        let parent_path = fixture.root.join("registry/archive");
        fs::set_permissions(&agent_path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(&parent_path).unwrap();
        fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o700)).unwrap();
        let parent_pinned =
            ManagedAgents::<Fake>::pinned_parent_directory(&parent_path, "agent archive").unwrap();
        fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ManagedAgents::<Fake>::verify_pinned_parent_directory(
            &parent_pinned,
            "agent archive",
        )
        .is_err());
    }

    #[test]
    fn atomic_snapshot_refuses_replaced_staging_and_installed_generations() {
        for phase in ["before-rename", "after-rename"] {
            let fixture = Fixture::new();
            fixture.make_legacy_dead();
            let manager = fixture.manager();
            let pinned = manager.pinned_agent_directory("foreign").unwrap();
            let active = fixture.root.join("registry/foreign");
            let held = active.join(format!(".held-{phase}"));
            let content = b"{\"new\":true}\n";
            let result = atomic_replace_bytes_with(
                &pinned,
                "output.json",
                content,
                (
                    |_pinned, temporary| {
                        if phase == "before-rename" {
                            fs::rename(active.join(temporary), &held).unwrap();
                            fs::write(active.join(temporary), vec![b'x'; content.len()]).unwrap();
                            fs::set_permissions(
                                active.join(temporary),
                                fs::Permissions::from_mode(0o600),
                            )
                            .unwrap();
                        }
                        Ok(())
                    },
                    |_pinned, name, temporary| {
                        if phase == "after-rename" {
                            fs::rename(active.join(name), &held).unwrap();
                            fs::write(active.join(name), b"replacement").unwrap();
                            fs::set_permissions(
                                active.join(name),
                                fs::Permissions::from_mode(0o600),
                            )
                            .unwrap();
                            fs::write(active.join(temporary), b"new-temp-owner").unwrap();
                            fs::set_permissions(
                                active.join(temporary),
                                fs::Permissions::from_mode(0o600),
                            )
                            .unwrap();
                        }
                        Ok(())
                    },
                ),
            );
            let error = result.unwrap_err().to_string();

            assert!(held.is_file());
            assert_eq!(fs::read(&held).unwrap(), content);
            if phase == "before-rename" {
                assert!(error.contains("staging generation changed"));
                assert!(error.contains("replacement was preserved"));
                assert!(!active.join("output.json").exists());
                let replacement = fs::read_dir(&active)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".output.json-recovery-")
                    })
                    .unwrap();
                assert_eq!(fs::read(replacement).unwrap(), vec![b'x'; content.len()]);
            } else {
                assert!(error.contains("was not the staged generation"));
                assert_eq!(
                    fs::read(active.join("output.json")).unwrap(),
                    b"replacement"
                );
                assert!(fs::read_dir(&active).unwrap().any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".output.json-recovery-")
                }));
            }
        }
    }

    #[test]
    fn legacy_recovery_never_restores_over_replaced_artifact_generations() {
        for phase in ["before-rename", "after-rename"] {
            let fixture = Fixture::new();
            let (_token, _digest, raw) = fixture.make_legacy_dead();
            let manager = fixture.manager();
            let record = manager.load("foreign").unwrap();
            let (_archive, destination) = manager.archive_destination(&record).unwrap();
            let pinned = manager.pinned_agent_directory("foreign").unwrap();
            let active = fixture.root.join("registry/foreign");
            let prior = b"{\"prior\":true}\n";
            fs::write(active.join("output.json"), prior).unwrap();
            fs::set_permissions(
                active.join("output.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            let held = active.join(format!(".held-recovery-{phase}"));

            let error = manager
                .publish_archive_with(
                    &pinned,
                    &destination,
                    &json!({"text":"new retained output"}),
                    &raw,
                    || Ok(()),
                    (
                        |_pinned, temporary| {
                            if phase == "before-rename" {
                                fs::rename(active.join(temporary), &held).unwrap();
                                fs::write(active.join(temporary), b"replacement owner").unwrap();
                                fs::set_permissions(
                                    active.join(temporary),
                                    fs::Permissions::from_mode(0o600),
                                )
                                .unwrap();
                            }
                            Ok(())
                        },
                        |_pinned, name, temporary| {
                            if phase == "after-rename" {
                                fs::rename(active.join(name), &held).unwrap();
                                fs::write(active.join(name), b"replacement owner").unwrap();
                                fs::set_permissions(
                                    active.join(name),
                                    fs::Permissions::from_mode(0o600),
                                )
                                .unwrap();
                                fs::write(active.join(temporary), b"new temp owner").unwrap();
                                fs::set_permissions(
                                    active.join(temporary),
                                    fs::Permissions::from_mode(0o600),
                                )
                                .unwrap();
                            }
                            Ok(())
                        },
                        rename_directory_noreplace_at,
                        || {},
                        || {},
                        |directory: &File, _label| directory.sync_all(),
                    ),
                )
                .unwrap_err();

            assert!(
                error.to_string().contains("generation changed")
                    || error.to_string().contains("replacement was preserved")
            );
            assert!(active.is_dir());
            assert!(!destination.exists());
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            if phase == "before-rename" {
                assert_eq!(fs::read(active.join("output.json")).unwrap(), prior);
            } else {
                assert_eq!(
                    fs::read(active.join("output.json")).unwrap(),
                    b"replacement owner"
                );
                assert!(held.is_file());
            }
        }
    }

    #[test]
    fn archive_publication_is_noreplace_rolls_back_output_and_reports_fsync_uncertainty() {
        let fixture = Fixture::new();
        let (_token, _digest, _) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let output = fixture.root.join("registry/foreign/output.json");
        let prior = b"{\n  \"prior\": \"exact evidence\"\n}\n";
        fs::write(&output, prior).unwrap();
        fs::set_permissions(&output, fs::Permissions::from_mode(0o600)).unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let expected_record = manager.record_bytes(&pinned).unwrap();
        std::os::unix::fs::symlink(fixture.root.join("missing"), &destination).unwrap();

        let error = manager
            .publish_archive(
                &pinned,
                &destination,
                &json!({"text":"new"}),
                &expected_record,
                || Ok(()),
            )
            .unwrap_err();
        assert!(error.to_string().contains("existing agent archive"));
        assert_eq!(fs::read(&output).unwrap(), prior);
        assert!(fixture.root.join("registry/foreign").is_dir());

        fs::remove_file(&destination).unwrap();
        let error = manager
            .publish_archive_with(
                &pinned,
                &destination,
                &json!({"text":"new retained output"}),
                &expected_record,
                || Ok(()),
                (
                    |_pinned, _temporary| Ok(()),
                    |_pinned, _name, _temporary| Ok(()),
                    rename_directory_noreplace_at,
                    || {},
                    || {},
                    |_directory: &File, _label| {
                        Err(io::Error::other("injected directory fsync failure"))
                    },
                ),
            )
            .unwrap_err();
        assert!(error.to_string().contains("published"));
        assert!(!fixture.root.join("registry/foreign").exists());
        assert!(destination.is_dir());
        assert_eq!(
            agent::read_private_json(&destination.join("output.json")).unwrap()["text"],
            "new retained output"
        );
    }

    #[test]
    fn publication_reconciles_successful_rename_reported_as_error() {
        for record_read_fails in [false, true] {
            let fixture = Fixture::new();
            let (_token, _digest, raw) = fixture.make_legacy_dead();
            let manager = fixture.manager();
            let record = manager.load("foreign").unwrap();
            let (_archive, destination) = manager.archive_destination(&record).unwrap();
            let active = fixture.root.join("registry/foreign");
            fs::write(active.join("output.json"), b"{\"prior\":true}\n").unwrap();
            fs::set_permissions(
                active.join("output.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            let pinned = manager.pinned_agent_directory("foreign").unwrap();

            let error = manager
                .publish_archive_with(
                    &pinned,
                    &destination,
                    &json!({"text":"new retained output"}),
                    &raw,
                    || Ok(()),
                    (
                        |_pinned, _temporary| Ok(()),
                        |_pinned, _name, _temporary| Ok(()),
                        |source_parent, source_name, destination_parent, destination_name| {
                            rename_directory_noreplace_at(
                                source_parent,
                                source_name,
                                destination_parent,
                                destination_name,
                            )?;
                            if record_read_fails {
                                fs::remove_file(destination.join("agent.json")).unwrap();
                                std::os::unix::fs::symlink(
                                    fixture.root.join("missing-agent-record"),
                                    destination.join("agent.json"),
                                )
                                .unwrap();
                            }
                            Err(fail("injected lost successful rename result"))
                        },
                        || {},
                        || {},
                        |directory: &File, _label| directory.sync_all(),
                    ),
                )
                .unwrap_err();

            assert!(error.to_string().contains("reported failure"));
            if record_read_fails {
                assert!(error.to_string().contains("record could not be proved"));
            } else {
                assert!(error.to_string().contains("was published"));
            }
            assert!(!active.exists());
            assert_eq!(
                agent::read_private_json(&destination.join("output.json")).unwrap()["text"],
                "new retained output"
            );
        }
    }

    #[test]
    fn publication_reconciles_late_destination_collision_and_restores_output() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let active = fixture.root.join("registry/foreign");
        let prior = b"{\"prior\":true}\n";
        fs::write(active.join("output.json"), prior).unwrap();
        fs::set_permissions(
            active.join("output.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();

        let error = manager
            .publish_archive(
                &pinned,
                &destination,
                &json!({"text":"new retained output"}),
                &raw,
                || {
                    fs::create_dir(&destination).unwrap();
                    fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
                    fs::write(destination.join("sentinel"), b"other owner").unwrap();
                    Ok(())
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("existing agent archive"));
        assert_eq!(fs::read(active.join("output.json")).unwrap(), prior);
        assert_eq!(
            fs::read(destination.join("sentinel")).unwrap(),
            b"other owner"
        );
    }

    #[test]
    fn publication_fsync_failure_attempts_both_and_retains_archive() {
        for failing_label in ["published agent archive", "published agent registry"] {
            let fixture = Fixture::new();
            let (_token, _digest, raw) = fixture.make_legacy_dead();
            let manager = fixture.manager();
            let record = manager.load("foreign").unwrap();
            let (_archive, destination) = manager.archive_destination(&record).unwrap();
            let pinned = manager.pinned_agent_directory("foreign").unwrap();
            let mut calls = Vec::new();

            let error = manager
                .publish_archive_with(
                    &pinned,
                    &destination,
                    &json!({"text":"new retained output"}),
                    &raw,
                    || Ok(()),
                    (
                        |_pinned, _temporary| Ok(()),
                        |_pinned, _name, _temporary| Ok(()),
                        rename_directory_noreplace_at,
                        || {},
                        || {},
                        |_directory, label| {
                            calls.push(label.to_owned());
                            if label == failing_label {
                                Err(io::Error::other("injected publication fsync failure"))
                            } else {
                                Ok(())
                            }
                        },
                    ),
                )
                .unwrap_err();

            assert!(error.to_string().contains("published"));
            assert_eq!(
                calls,
                ["published agent archive", "published agent registry"]
            );
            assert!(!fixture.root.join("registry/foreign").exists());
            assert_eq!(
                agent::read_private_json(&destination.join("output.json")).unwrap()["text"],
                "new retained output"
            );
        }
    }

    #[test]
    fn pinned_publication_refuses_postproof_directory_swap() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let active = fixture.root.join("registry/foreign");
        let displaced = fixture.root.join("registry/.foreign-original");
        let mut published = false;

        fs::rename(&active, &displaced).unwrap();
        fs::create_dir(&active).unwrap();
        fs::set_permissions(&active, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(active.join("agent.json"), &raw).unwrap();
        fs::set_permissions(active.join("agent.json"), fs::Permissions::from_mode(0o600)).unwrap();

        let error = manager
            .publish_pinned_directory_with(
                &pinned,
                &destination,
                &raw,
                &mut published,
                (
                    rename_directory_noreplace_at,
                    || {},
                    || {},
                    |directory: &File, _label| directory.sync_all(),
                ),
            )
            .unwrap_err();
        assert!(!published);

        assert!(error.to_string().contains("registry directory changed"));
        assert!(active.join("agent.json").is_file());
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn pinned_publication_refuses_reappeared_source_and_reports_published_state() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let active = fixture.root.join("registry/foreign");

        let error = manager
            .publish_archive_with(
                &pinned,
                &destination,
                &json!({"text":"new retained output"}),
                &raw,
                || Ok(()),
                (
                    |_pinned, _temporary| Ok(()),
                    |_pinned, _name, _temporary| Ok(()),
                    rename_directory_noreplace_at,
                    || {
                        fs::create_dir(&active).unwrap();
                        fs::set_permissions(&active, fs::Permissions::from_mode(0o700)).unwrap();
                        fs::write(active.join("agent.json"), &raw).unwrap();
                        fs::set_permissions(
                            active.join("agent.json"),
                            fs::Permissions::from_mode(0o600),
                        )
                        .unwrap();
                    },
                    || {},
                    |directory: &File, _label| directory.sync_all(),
                ),
            )
            .unwrap_err();

        assert!(error.to_string().contains("rollback was not attempted"));
        assert!(active.is_dir());
        assert!(destination.is_dir());
        assert_eq!(
            agent::read_private_json(&destination.join("output.json")).unwrap()["text"],
            "new retained output"
        );
    }

    #[test]
    fn pinned_publication_refuses_postrename_permission_change() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();

        let error = manager
            .publish_archive_with(
                &pinned,
                &destination,
                &json!({"text":"new retained output"}),
                &raw,
                || Ok(()),
                (
                    |_pinned, _temporary| Ok(()),
                    |_pinned, _name, _temporary| Ok(()),
                    rename_directory_noreplace_at,
                    || {
                        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
                            .unwrap();
                    },
                    || {},
                    |directory: &File, _label| directory.sync_all(),
                ),
            )
            .unwrap_err();

        assert!(error.to_string().contains("not a private directory"));
        assert!(!fixture.root.join("registry/foreign").exists());
        assert_eq!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            agent::read_private_json(&destination.join("output.json")).unwrap()["text"],
            "new retained output"
        );
    }

    #[test]
    fn pinned_publication_never_promotes_replaced_archive_entry_during_rollback() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let active = fixture.root.join("registry/foreign");
        let displaced = fixture.root.join("registry/archive/.held-original");

        let error = manager
            .publish_archive_with(
                &pinned,
                &destination,
                &json!({"text":"new retained output"}),
                &raw,
                || Ok(()),
                (
                    |_pinned, _temporary| Ok(()),
                    |_pinned, _name, _temporary| Ok(()),
                    rename_directory_noreplace_at,
                    || {
                        fs::rename(&destination, &displaced).unwrap();
                        fs::create_dir(&destination).unwrap();
                        fs::set_permissions(&destination, fs::Permissions::from_mode(0o700))
                            .unwrap();
                        fs::write(destination.join("attacker-marker"), b"replacement").unwrap();
                    },
                    || {},
                    |directory: &File, _label| directory.sync_all(),
                ),
            )
            .unwrap_err();

        assert!(error.to_string().contains("destination was replaced"));
        assert!(!active.exists());
        assert_eq!(
            fs::read(destination.join("attacker-marker")).unwrap(),
            b"replacement"
        );
        assert_eq!(
            agent::read_private_json(&displaced.join("output.json")).unwrap()["text"],
            "new retained output"
        );
    }

    #[test]
    fn pinned_publication_reproves_generation_after_inverse_rollback() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let active = fixture.root.join("registry/foreign");
        let displaced = fixture.root.join("registry/.foreign-after-rollback");

        let mut wrong_record = raw.clone();
        wrong_record.extend_from_slice(b"mismatch");
        let error = manager
            .publish_archive_with(
                &pinned,
                &destination,
                &json!({"text":"new retained output"}),
                &wrong_record,
                || Ok(()),
                (
                    |_pinned, _temporary| Ok(()),
                    |_pinned, _name, _temporary| Ok(()),
                    rename_directory_noreplace_at,
                    || {},
                    || fs::rename(&active, &displaced).unwrap(),
                    |directory: &File, _label| directory.sync_all(),
                ),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("rollback state could not be proved"));
        assert!(!active.exists());
        assert!(!destination.exists());
        assert_eq!(
            agent::read_private_json(&displaced.join("output.json")).unwrap()["text"],
            "new retained output"
        );
    }

    #[test]
    fn pinned_publication_refuses_replaced_archive_parent_and_rolls_back() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let active = fixture.root.join("registry/foreign");
        let displaced_archive = fixture.root.join("registry/.archive-original");
        let mut published = false;

        let error = manager
            .publish_pinned_directory_with(
                &pinned,
                &destination,
                &raw,
                &mut published,
                (
                    rename_directory_noreplace_at,
                    || {
                        fs::rename(&archive, &displaced_archive).unwrap();
                        fs::create_dir(&archive).unwrap();
                        fs::set_permissions(&archive, fs::Permissions::from_mode(0o700)).unwrap();
                    },
                    || {},
                    |directory: &File, _label| directory.sync_all(),
                ),
            )
            .unwrap_err();

        assert!(published);
        assert!(error.to_string().contains("agent archive changed"));
        assert!(active.is_dir());
        assert!(!destination.exists());
        assert!(!displaced_archive
            .join(destination.file_name().unwrap())
            .exists());
    }

    #[test]
    fn pinned_publication_reports_each_rollback_fsync_failure() {
        for failing_label in ["agent archive rollback", "agent registry rollback"] {
            let fixture = Fixture::new();
            let (_token, _digest, raw) = fixture.make_legacy_dead();
            let manager = fixture.manager();
            let record = manager.load("foreign").unwrap();
            let (_archive, destination) = manager.archive_destination(&record).unwrap();
            let pinned = manager.pinned_agent_directory("foreign").unwrap();
            let active_record = fixture.root.join("registry/foreign/agent.json");
            let published_record = destination.join("agent.json");
            let mut calls = Vec::new();

            let error = manager
                .publish_archive_with(
                    &pinned,
                    &destination,
                    &json!({"text":"new retained output"}),
                    &raw,
                    || Ok(()),
                    (
                        |_pinned, _temporary| Ok(()),
                        |_pinned, _name, _temporary| Ok(()),
                        rename_directory_noreplace_at,
                        || {
                            fs::write(&published_record, b"mismatch").unwrap();
                            fs::set_permissions(
                                &published_record,
                                fs::Permissions::from_mode(0o600),
                            )
                            .unwrap();
                        },
                        || {
                            fs::write(&active_record, &raw).unwrap();
                            fs::set_permissions(&active_record, fs::Permissions::from_mode(0o600))
                                .unwrap();
                        },
                        |_directory, label| {
                            calls.push(label.to_owned());
                            if label == failing_label {
                                Err(io::Error::other("injected rollback fsync failure"))
                            } else {
                                Ok(())
                            }
                        },
                    ),
                )
                .unwrap_err();

            let message = error.to_string();
            assert!(message.contains("rollback completed"));
            assert!(message.contains("durability is uncertain"));
            assert!(message.contains(failing_label));
            assert!(fixture.root.join("registry/foreign").is_dir());
            assert!(!destination.exists());
            assert_eq!(calls, ["agent archive rollback", "agent registry rollback"]);
            assert_eq!(
                agent::read_private_json(&fixture.root.join("registry/foreign/output.json"))
                    .unwrap()["text"],
                "new retained output"
            );
        }
    }

    #[test]
    fn pinned_publication_refuses_postproof_record_swap() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let record_path = fixture.root.join("registry/foreign/agent.json");
        let mut published = false;

        let mut document = agent::read_private_json(&record_path).unwrap();
        document["goal"] = json!("replacement record");
        agent::atomic_json(&record_path, &document).unwrap();

        let error = manager
            .publish_pinned_directory_with(
                &pinned,
                &destination,
                &raw,
                &mut published,
                (
                    rename_directory_noreplace_at,
                    || {},
                    || {},
                    |directory: &File, _label| directory.sync_all(),
                ),
            )
            .unwrap_err();
        assert!(published);

        assert!(error
            .to_string()
            .contains("record changed during archival publication"));
        assert!(fixture.root.join("registry/foreign").is_dir());
        assert!(!destination.exists());
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn pinned_publication_consumes_original_generation_after_safe_aba() {
        let fixture = Fixture::new();
        let (_token, _digest, raw) = fixture.make_legacy_dead();
        let manager = fixture.manager();
        let record = manager.load("foreign").unwrap();
        let (_archive, destination) = manager.archive_destination(&record).unwrap();
        let pinned = manager.pinned_agent_directory("foreign").unwrap();
        let active = fixture.root.join("registry/foreign");
        let held = fixture.root.join("registry/.foreign-held");
        let mut published = false;

        fs::rename(&active, &held).unwrap();
        fs::create_dir(&active).unwrap();
        fs::set_permissions(&active, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(active.join("agent.json"), &raw).unwrap();
        fs::remove_file(active.join("agent.json")).unwrap();
        fs::remove_dir(&active).unwrap();
        fs::rename(&held, &active).unwrap();
        manager
            .publish_pinned_directory_with(
                &pinned,
                &destination,
                &raw,
                &mut published,
                (
                    rename_directory_noreplace_at,
                    || {},
                    || {},
                    |directory: &File, _label| directory.sync_all(),
                ),
            )
            .unwrap();
        assert!(published);

        assert_eq!(
            fs::symlink_metadata(&destination).unwrap().ino(),
            pinned.inode
        );
        assert!(!active.exists());
    }

    #[test]
    fn explicit_legacy_recovery_preflights_archive_and_snapshot_failures() {
        for case in ["collision", "snapshot"] {
            let fixture = Fixture::new();
            let (token, digest, raw) = fixture.make_legacy_dead();
            let active = fixture.root.join("registry/foreign");
            if case == "collision" {
                let destination = fixture
                    .root
                    .join(format!("registry/archive/foreign-{token}"));
                fs::create_dir_all(&destination).unwrap();
                fs::set_permissions(
                    destination.parent().unwrap(),
                    fs::Permissions::from_mode(0o700),
                )
                .unwrap();
                fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
            } else {
                fs::set_permissions(&active, fs::Permissions::from_mode(0o500)).unwrap();
            }
            let result = fixture.manager().stop_with_options(
                "foreign",
                StopOptions {
                    expected_token: Some(token),
                    recover_legacy_adoption: true,
                    expected_record_sha256: Some(digest),
                },
            );
            fs::set_permissions(&active, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(result.is_err(), "{case} unexpectedly succeeded");
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert_eq!(fs::read(active.join("agent.json")).unwrap(), raw);
        }
    }

    #[test]
    fn stopping_live_adopted_agents_refuses_replaced_shell_generations() {
        let original = Fake::foreign_shell_identity();
        let mut replacements = Vec::new();
        let mut new_pane_shell = original.clone();
        new_pane_shell.pid += 1;
        replacements.push(new_pane_shell);
        let mut reused_pid = original.clone();
        reused_pid.starttime_ticks += 1;
        replacements.push(reused_pid);
        let mut replaced_image = original;
        replaced_image.executable_inode += 1;
        replacements.push(replaced_image);

        for replacement in replacements {
            let fixture = Fixture::new();
            fixture.adopt();
            *fixture.client.foreign_shell_identity.lock().unwrap() = replacement;

            let error = fixture.manager().stop("foreign").unwrap_err();

            assert!(error
                .to_string()
                .contains("recorded pane shell generation changed"));
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.client.started.load(Ordering::Relaxed));
            assert_eq!(
                fixture.manager().load("foreign").unwrap().lifecycle,
                "running"
            );
        }
    }

    #[test]
    fn stopping_a_sessionless_live_adopted_agent_refuses_replaced_shell_generation() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let adopted = fixture.adopt();
        assert!(adopted["session_value"].is_null());
        fixture
            .client
            .foreign_shell_identity
            .lock()
            .unwrap()
            .starttime_ticks += 1;

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error
            .to_string()
            .contains("recorded pane shell generation changed"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert!(fixture.client.started.load(Ordering::Relaxed));
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_a_live_legacy_adopted_agent_refuses_missing_shell_identity() {
        let fixture = Fixture::new();
        fixture.adopt();
        let path = fixture.root.join("registry/foreign/agent.json");
        let mut document = fixture.manager().load("foreign").unwrap().public_value();
        document
            .as_object_mut()
            .unwrap()
            .remove("foreign_shell_identity");
        agent::atomic_json(&path, &document).unwrap();

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error
            .to_string()
            .contains("legacy record has no identity-bound"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert!(fixture.client.started.load(Ordering::Relaxed));
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_a_live_adopted_agent_refuses_shell_change_during_capture() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture
            .client
            .change_foreign_shell_on_read
            .store(true, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error
            .to_string()
            .contains("identity could not be reverified"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert!(fixture.client.started.load(Ordering::Relaxed));
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_an_absent_adopted_agent_refuses_shell_change_during_capture() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture
            .client
            .change_foreign_shell_on_read
            .store(true, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error
            .to_string()
            .contains("identity could not be reverified"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_an_absent_adopted_agent_refuses_cwd_change() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture
            .client
            .wrong_foreign_cwd
            .store(true, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error.to_string().contains("workspace, or cwd changed"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_an_absent_adopted_agent_refuses_a_moved_tab() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture.client.panes.lock().unwrap()[0].tab_id = "moved-tab".to_owned();

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error
            .to_string()
            .contains("recorded tab changed while the agent was absent"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_a_revalidated_live_adopted_agent_allows_a_moved_tab() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture.client.panes.lock().unwrap()[0].tab_id = "moved-tab".to_owned();

        let stopped = fixture.manager().stop("foreign").unwrap();

        assert_eq!(stopped["runtime_preserved"], true);
        assert_eq!(stopped["pane_closed"], false);
        assert_eq!(stopped["tab_closed"], false);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(fixture.client.panes.lock().unwrap()[0].tab_id, "moved-tab");
        assert!(fixture.manager().list().unwrap().is_empty());
    }

    #[test]
    fn stopping_an_adopted_agent_refuses_missing_or_duplicated_recorded_pane() {
        for presentation_count in [0, 2] {
            let fixture = Fixture::new();
            fixture.adopt();
            let original = fixture.client.panes.lock().unwrap()[0].clone();
            let mut panes = fixture.client.panes.lock().unwrap();
            panes.clear();
            if presentation_count == 2 {
                panes.push(original.clone());
                let mut duplicate = original;
                duplicate.tab_id = "duplicate-tab".to_owned();
                panes.push(duplicate);
            }
            drop(panes);

            let error = fixture.manager().stop("foreign").unwrap_err();

            assert!(error.to_string().contains(&format!(
                "expected one recorded pane, found {presentation_count}"
            )));
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert_eq!(
                fixture.manager().load("foreign").unwrap().lifecycle,
                "running"
            );
        }
    }

    #[test]
    fn stopping_a_live_adopted_agent_refuses_a_replaced_native_session() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture
            .client
            .change_session_after_save
            .store(true, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error.to_string().contains("exactly one live pane"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_a_dead_adopted_agent_refuses_leaving_idle_during_capture() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture
            .client
            .leave_idle_shell_on_read
            .store(true, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();
        let message = error.to_string();

        assert!(message.contains("identity could not be reverified"));
        assert!(message.contains("identity-bound idle shell"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_a_dead_adopted_agent_refuses_restart_during_capture() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture.client.started.store(false, Ordering::Relaxed);
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture
            .client
            .restart_foreign_on_read
            .store(true, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error.to_string().contains("runtime identity changed"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn stopping_a_sessionless_adopted_agent_refuses_a_new_session_during_capture() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture.adopt();
        fixture
            .client
            .restart_foreign_on_read
            .store(true, Ordering::Relaxed);

        let error = fixture.manager().stop("foreign").unwrap_err();

        assert!(error
            .to_string()
            .contains("runtime identity changed during output capture"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(
            fixture.manager().load("foreign").unwrap().lifecycle,
            "running"
        );
    }

    #[test]
    fn adoption_refuses_identity_mismatches_without_creating_a_record() {
        let fixture = Fixture::new();
        fixture
            .client
            .panes
            .lock()
            .unwrap()
            .push(Fake::pane("owned"));
        let manager = fixture.manager();
        assert!(manager.adopt("foreign", fixture.adopt_options()).is_err());
        assert!(!fixture.root.join("registry/foreign").exists());

        let fixture = Fixture::new();
        fixture.prepare_foreign();
        let mut options = fixture.adopt_options();
        options.harness = "claude".to_owned();
        assert!(fixture.manager().adopt("foreign", options).is_err());
        assert!(!fixture.root.join("registry/foreign").exists());

        let fixture = Fixture::new();
        fixture.prepare_foreign();
        let mut options = fixture.adopt_options();
        options.expected_workspace = "wrong".to_owned();
        assert!(fixture.manager().adopt("foreign", options).is_err());
        assert!(!fixture.root.join("registry/foreign").exists());

        let fixture = Fixture::new();
        fixture.prepare_foreign();
        let wrong = fixture.root.join("wrong");
        fs::create_dir(&wrong).unwrap();
        let mut options = fixture.adopt_options();
        options.cwd = wrong;
        assert!(fixture.manager().adopt("foreign", options).is_err());
        assert!(!fixture.root.join("registry/foreign").exists());

        let fixture = Fixture::new();
        fixture.prepare_foreign();
        let mut options = fixture.adopt_options();
        options.session = Some("wrong-thread".to_owned());
        assert!(fixture.manager().adopt("foreign", options).is_err());
        assert!(!fixture.root.join("registry/foreign").exists());
    }

    #[test]
    fn adoption_refuses_a_duplicate_pane_and_preserves_the_first_generation() {
        let fixture = Fixture::new();
        let adopted = fixture.adopt();
        let error = fixture
            .manager()
            .adopt("second", fixture.adopt_options())
            .unwrap_err();
        assert!(error.to_string().contains("already registered as"));
        assert_eq!(
            fixture.manager().get("foreign").unwrap()["token"],
            adopted["token"]
        );
        assert!(!fixture.root.join("registry/second").exists());
    }

    #[test]
    fn same_provider_local_session_id_is_allowed_for_different_harnesses() {
        let fixture = Fixture::new();
        fixture.adopt();
        let mut pane = Fake::pane("claude");
        pane.tab_id = "claude-tab".to_owned();
        fixture.client.panes.lock().unwrap().push(pane);
        let mut options = fixture.adopt_options();
        options.pane_id = "claude".to_owned();
        options.harness = "claude".to_owned();
        let adopted = fixture.manager().adopt("claude", options).unwrap();
        assert_eq!(adopted["session_agent"], "claude");
        assert_eq!(adopted["session_value"], "thread");
        assert_eq!(fixture.manager().list().unwrap().len(), 2);
    }

    #[test]
    fn adoption_refuses_same_harness_session_held_by_headless_record() {
        let fixture = Fixture::new();
        fixture.start(None);
        let path = fixture.root.join("registry/worker/agent.json");
        let mut document = agent::read_private_json(&path).unwrap();
        document["launch"]["adapter"] = json!("turn-runner");
        document["launch"]["mode"] = json!("headless");
        document["launch"]["backend"] = json!("tmux");
        document["launch"]["runtime_home"] = json!(fixture.root.join("runtime"));
        document["pane_id"] = json!("headless-pane");
        agent::atomic_json(&path, &document).unwrap();
        *fixture.client.panes.lock().unwrap() = vec![Fake::pane("foreign")];
        fixture
            .client
            .duplicate_session
            .store(true, Ordering::Relaxed);
        let mut options = fixture.adopt_options();
        options.pane_id = "foreign".to_owned();
        let error = fixture.manager().adopt("foreign", options).unwrap_err();
        assert!(error.to_string().contains("already registered as"));
        assert!(!fixture.root.join("registry/foreign").exists());
    }

    #[test]
    fn interactive_start_refuses_resume_claimed_by_adopted_agent() {
        let fixture = Fixture::new();
        fixture.adopt();
        let presentation = Fake::pane("owned");
        let error = fixture
            .manager()
            .start(
                "second",
                &fixture.root,
                StartOptions {
                    resume: Some("thread".to_owned()),
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("already registered as"));
        assert!(!fixture.root.join("registry/second").exists());
        assert_eq!(*fixture.client.panes.lock().unwrap(), [presentation]);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn start_refuses_unregistered_same_session_in_another_workspace_and_is_stoppable() {
        let fixture = Fixture::new();
        let mut external = Fake::pane("external");
        external.tab_id = "external-tab".to_owned();
        external.workspace_id = "other-workspace".to_owned();
        fixture.client.panes.lock().unwrap().push(external.clone());
        fixture
            .client
            .duplicate_session
            .store(true, Ordering::Relaxed);
        let error = fixture
            .manager()
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("not globally unique"), "{error}");
        let failed = fixture.manager().load("worker").unwrap();
        assert_eq!(failed.lifecycle, "launch_failed");
        assert!(failed.session_value.is_none());
        assert_eq!(
            fixture.manager().stop("worker").unwrap()["pane_closed"],
            true
        );
        assert_eq!(*fixture.client.panes.lock().unwrap(), [external]);
    }

    #[test]
    fn conflicting_started_pane_remains_stoppable_if_initial_close_fails() {
        let fixture = Fixture::new();
        fixture.start(None);
        let holder_path = fixture.root.join("registry/worker/agent.json");
        let mut holder = agent::read_private_json(&holder_path).unwrap();
        holder["launch"]["adapter"] = json!("turn-runner");
        holder["launch"]["mode"] = json!("headless");
        holder["launch"]["backend"] = json!("tmux");
        holder["launch"]["runtime_home"] = json!(fixture.root.join("runtime"));
        holder["pane_id"] = json!("headless-pane");
        agent::atomic_json(&holder_path, &holder).unwrap();
        fixture.client.panes.lock().unwrap().clear();
        fixture.client.fail_close.store(true, Ordering::Relaxed);
        let error = fixture
            .manager()
            .start(
                "second",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("could not close"), "{error}");
        let failed = fixture.manager().load("second").unwrap();
        assert_eq!(failed.lifecycle, "launch_failed");
        assert!(failed.session_agent.is_none() && failed.session_value.is_none());
        fixture.client.fail_close.store(false, Ordering::Relaxed);
        assert_eq!(
            fixture.manager().stop("second").unwrap()["pane_closed"],
            true
        );
    }

    #[test]
    fn adoption_refuses_a_reported_session_visible_in_two_live_panes() {
        let fixture = Fixture::new();
        fixture.prepare_foreign();
        let mut duplicate = Fake::pane("duplicate");
        duplicate.tab_id = "other-tab".to_owned();
        fixture.client.panes.lock().unwrap().push(duplicate);
        fixture
            .client
            .duplicate_session
            .store(true, Ordering::Relaxed);
        let error = fixture
            .manager()
            .adopt("foreign", fixture.adopt_options())
            .unwrap_err();
        assert!(error.to_string().contains("exactly one live pane"));
        assert!(!fixture.root.join("registry/foreign").exists());
    }

    #[test]
    fn adoption_archives_failed_generation_if_identity_changes_after_save() {
        for initially_reported in [true, false] {
            let fixture = Fixture::new();
            fixture.prepare_foreign();
            fixture
                .client
                .report_session
                .store(initially_reported, Ordering::Relaxed);
            fixture
                .client
                .change_session_after_save
                .store(true, Ordering::Relaxed);
            let error = fixture
                .manager()
                .adopt("foreign", fixture.adopt_options())
                .unwrap_err();
            assert!(error.to_string().contains("was not registered"));
            assert!(!fixture.root.join("registry/foreign").exists());
            let records: Vec<PathBuf> = fs::read_dir(fixture.root.join("registry/archive"))
                .unwrap()
                .map(|entry| entry.unwrap().path().join("agent.json"))
                .collect();
            assert_eq!(records.len(), 1);
            let saved = agent::read_private_json(&records[0]).unwrap();
            assert_eq!(saved["lifecycle"], "adopt_failed");
            assert!(saved["error"]
                .as_str()
                .is_some_and(|value| !value.is_empty()));
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.client.runs.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn sessionless_adoption_stays_pane_bound_after_goal_session_binding() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let adopted = fixture.adopt();
        let manager = fixture.manager();
        assert!(adopted["session_value"].is_null());
        manager
            .bind_session("foreign", "explicit-thread", None)
            .unwrap();
        manager
            .send("foreign", "still pane bound", DrainOptions::default())
            .unwrap();
        let binding =
            agent::read_private_json(&fixture.root.join("registry/foreign/queue/target.json"))
                .unwrap();
        assert_eq!(binding["kind"], "pane");
        assert_eq!(binding["pane_id"], "owned");
    }

    #[test]
    fn bind_session_refuses_an_authoritative_owner_in_another_record() {
        let fixture = Fixture::new();
        fixture.adopt();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let mut second = Fake::pane("second");
        second.tab_id = "second-tab".to_owned();
        fixture.client.panes.lock().unwrap().push(second);
        let mut options = fixture.adopt_options();
        options.pane_id = "second".to_owned();
        fixture.manager().adopt("second", options).unwrap();
        let error = fixture
            .manager()
            .bind_session("second", "thread", None)
            .unwrap_err();
        assert!(error.to_string().contains("already registered as"));
        assert!(fixture
            .manager()
            .load("second")
            .unwrap()
            .session_value
            .is_none());
    }

    #[test]
    fn bind_session_refuses_unregistered_live_session_contradiction() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture.adopt();
        let mut reported = Fake::pane("reported");
        reported.tab_id = "reported-tab".to_owned();
        fixture.client.panes.lock().unwrap().push(reported);
        let error = fixture
            .manager()
            .bind_session("foreign", "thread", None)
            .unwrap_err();
        assert!(error.to_string().contains("another or ambiguous live pane"));
        assert!(fixture
            .manager()
            .load("foreign")
            .unwrap()
            .session_value
            .is_none());
    }

    #[test]
    fn concurrent_bind_session_allows_exactly_one_provider_local_owner() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        fixture.adopt();
        let mut second = Fake::pane("second");
        second.tab_id = "second-tab".to_owned();
        fixture.client.panes.lock().unwrap().push(second);
        let mut options = fixture.adopt_options();
        options.pane_id = "second".to_owned();
        let manager = fixture.manager();
        manager.adopt("second", options).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        std::thread::scope(|scope| {
            for name in ["foreign", "second"] {
                let barrier = Arc::clone(&barrier);
                let outcomes = Arc::clone(&outcomes);
                let manager = &manager;
                scope.spawn(move || {
                    barrier.wait();
                    let bound = manager.bind_session(name, "shared-thread", None).is_ok();
                    outcomes.lock().unwrap().push(bound);
                });
            }
            barrier.wait();
        });
        let mut outcomes = outcomes.lock().unwrap().clone();
        outcomes.sort_unstable();
        assert_eq!(outcomes, [false, true]);
        let claims = [
            manager.load("foreign").unwrap().session_value,
            manager.load("second").unwrap().session_value,
        ];
        assert_eq!(
            claims
                .iter()
                .filter(|claim| claim.as_deref() == Some("shared-thread"))
                .count(),
            1
        );
    }

    #[test]
    fn failed_probe_after_allocation_preserves_exact_pane_for_cleanup() {
        let fixture = Fixture::new();
        fixture.client.fail_panes.store(true, Ordering::Relaxed);
        let manager = fixture.manager();
        assert!(manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    ..StartOptions::default()
                }
            )
            .is_err());
        let record = manager.get("worker").unwrap();
        assert_eq!(record["pane_id"], "owned");
        assert_eq!(record["tab_id"], "tab");
        assert_eq!(record["lifecycle"], "launch_failed");
        fixture.client.fail_panes.store(false, Ordering::Relaxed);
        assert_eq!(manager.stop("worker").unwrap()["pane_closed"], true);
    }

    #[test]
    fn failed_muse_launch_with_our_stale_pane_report_is_stoppable() {
        let fixture = Fixture::new();
        fixture
            .client
            .custom_dies_after_report
            .store(true, Ordering::Relaxed);
        let manager = fixture.manager();
        let error = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("foreground process"));
        let record = manager.load("worker").unwrap();
        assert_eq!(record.lifecycle, "launch_failed");
        assert!(record.pane_reported_by_agentctl);
        assert_eq!(manager.stop("worker").unwrap()["pane_closed"], true);
    }

    #[test]
    fn muse_identity_is_persisted_before_a_trust_failure_and_allows_stop() {
        let fixture = Fixture::new();
        fixture
            .client
            .custom_fails_after_identity
            .store(true, Ordering::Relaxed);
        let manager = fixture.manager();
        let error = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("trust prompt"));
        let record = manager.load("worker").unwrap();
        assert_eq!(record.lifecycle, "launch_failed");
        assert_eq!(
            record.custom_process_identity,
            Some(Fake::custom_identity())
        );
        assert!(!record.pane_reported_by_agentctl);
        assert_eq!(manager.stop("worker").unwrap()["pane_closed"], true);
    }

    #[test]
    fn persisted_muse_identity_makes_a_starting_record_stoppable() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let mut record = manager.load("worker").unwrap();
        assert!(record.custom_process_identity.is_some());
        record.lifecycle = "starting".to_owned();
        manager.save(&record).unwrap();
        assert_eq!(manager.stop("worker").unwrap()["pane_closed"], true);
    }

    #[test]
    fn starting_muse_without_report_or_identity_is_not_stoppable() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let mut record = manager.load("worker").unwrap();
        record.lifecycle = "starting".to_owned();
        record.pane_reported_by_agentctl = false;
        record.custom_process_identity = None;
        manager.save(&record).unwrap();
        let error = manager.stop("worker").unwrap_err();
        assert!(error.to_string().contains("cannot prove starting"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn running_owned_dead_muse_requires_token_then_archives_stale_label_pane() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        fixture.client.custom_alive.store(false, Ordering::Relaxed);
        let objective = "recover the retained timerfd worktree";
        let queue = fixture.root.join("registry/worker/queue");
        let identifier =
            agent::enqueue(&queue, &goal_prompt("muse", objective), Some("legacy-goal")).unwrap();
        let artifact_path = queue.join("inbox").join(format!("{identifier}.json"));
        let mut artifact = agent::read_private_json(&artifact_path).unwrap();
        artifact.as_object_mut().unwrap().remove("kind");
        agent::atomic_json(&artifact_path, &artifact).unwrap();
        let record_path = fixture.root.join("registry/worker/agent.json");
        let mut legacy =
            downgrade_current_record_to_v2(agent::read_private_json(&record_path).unwrap());
        legacy["goal"] = json!(objective);
        legacy["goal_delivery"] = json!("possibly_submitted");
        legacy["goal_message_id"] = json!(identifier);
        legacy["goal_messages"] = json!({});
        agent::atomic_json(&record_path, &legacy).unwrap();

        let error = manager.stop("worker").unwrap_err();
        assert!(error.to_string().contains("requires --expected-token"));
        let stopped = manager
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(started["token"].as_str().unwrap().to_owned()),
                    ..StopOptions::default()
                },
            )
            .unwrap();
        assert_eq!(stopped["managed_dead"], true);
        assert_eq!(stopped["pane_closed"], false);
        assert_eq!(stopped["runtime_preserved"], true);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(*fixture.client.panes.lock().unwrap(), [Fake::pane("owned")]);
        let archive = PathBuf::from(stopped["archive"].as_str().unwrap());
        assert_eq!(
            agent::read_private_json(
                &archive
                    .join("queue/inbox")
                    .join(format!("{identifier}.json")),
            )
            .unwrap()["kind"],
            "goal"
        );
    }

    #[test]
    fn running_owned_dead_muse_refuses_unproved_or_changed_pane() {
        for case in ["descendant", "moved", "label"] {
            let fixture = Fixture::new();
            fixture
                .client
                .report_session
                .store(false, Ordering::Relaxed);
            let manager = fixture.manager();
            let started = manager
                .start(
                    "worker",
                    &fixture.root,
                    StartOptions {
                        workspace_id: Some("workspace".to_owned()),
                        harness: "muse".to_owned(),
                        ..StartOptions::default()
                    },
                )
                .unwrap();
            fixture.client.custom_alive.store(false, Ordering::Relaxed);
            match case {
                "descendant" => fixture
                    .client
                    .custom_at_idle_shell
                    .store(false, Ordering::Relaxed),
                "moved" => fixture.client.panes.lock().unwrap()[0].tab_id = "moved".to_owned(),
                "label" => fixture
                    .client
                    .custom_reported
                    .store(false, Ordering::Relaxed),
                _ => unreachable!(),
            }
            assert!(
                manager
                    .stop_with_options(
                        "worker",
                        StopOptions {
                            expected_token: Some(started["token"].as_str().unwrap().to_owned()),
                            ..StopOptions::default()
                        },
                    )
                    .is_err(),
                "case {case}"
            );
            assert!(fixture.client.closed.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn running_owned_dead_muse_refuses_shell_generation_change() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        fixture.client.custom_alive.store(false, Ordering::Relaxed);
        fixture
            .client
            .change_foreign_shell_on_read
            .store(true, Ordering::Relaxed);

        let error = manager
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(started["token"].as_str().unwrap().to_owned()),
                    ..StopOptions::default()
                },
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("runtime identity changed"),
            "{error}"
        );
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn running_owned_dead_muse_never_closes_a_replacement_terminal() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        fixture.client.custom_alive.store(false, Ordering::Relaxed);
        fixture.client.panes.lock().unwrap()[0].terminal_id =
            Some("replacement-terminal".to_owned());

        let stopped = manager
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(started["token"].as_str().unwrap().to_owned()),
                    ..StopOptions::default()
                },
            )
            .unwrap();

        assert_eq!(stopped["managed_dead"], true);
        assert_eq!(stopped["pane_closed"], false);
        assert_eq!(stopped["runtime_preserved"], true);
        assert!(stopped["continuation"]
            .as_str()
            .unwrap()
            .contains("conditional close"));
        assert_eq!(
            fixture.client.panes.lock().unwrap()[0]
                .terminal_id
                .as_deref(),
            Some("replacement-terminal")
        );
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn running_owned_dead_muse_retries_stopped_publication_without_touching_pane() {
        for replace_terminal in [false, true] {
            let fixture = Fixture::new();
            fixture
                .client
                .report_session
                .store(false, Ordering::Relaxed);
            let manager = fixture.manager();
            let started = manager
                .start(
                    "worker",
                    &fixture.root,
                    StartOptions {
                        workspace_id: Some("workspace".to_owned()),
                        harness: "muse".to_owned(),
                        ..StartOptions::default()
                    },
                )
                .unwrap();
            fixture.client.custom_alive.store(false, Ordering::Relaxed);
            let active = fixture.root.join("registry/worker");
            let mut stopped = manager.load("worker").unwrap();
            stopped.lifecycle = "stopped".to_owned();
            agent::atomic_json(
                &active.join("agent.json"),
                &stopped.storage_value().unwrap(),
            )
            .unwrap();
            let stopped_bytes = fs::read(active.join("agent.json")).unwrap();
            agent::atomic_json(
                &active.join(MANAGED_DEAD_RETIREMENT_FILE),
                &ManagedAgents::<Fake>::managed_dead_retirement_value(&stopped, &stopped_bytes),
            )
            .unwrap();
            if replace_terminal {
                fixture.client.panes.lock().unwrap()[0].terminal_id =
                    Some("replacement-terminal".to_owned());
            }
            let retained = fixture.client.panes.lock().unwrap().clone();

            let result = manager
                .stop_with_options(
                    "worker",
                    StopOptions {
                        expected_token: Some(started["token"].as_str().unwrap().to_owned()),
                        ..StopOptions::default()
                    },
                )
                .unwrap();

            assert_eq!(result["pane_closed"], false);
            assert_eq!(result["runtime_preserved"], true);
            assert_eq!(*fixture.client.panes.lock().unwrap(), retained);
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(!active.exists());
        }
    }

    #[test]
    fn running_owned_dead_muse_stop_result_loss_reconciles_from_archive_receipt() {
        let fixture = Fixture::new();
        fixture
            .client
            .report_session
            .store(false, Ordering::Relaxed);
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        fixture.client.custom_alive.store(false, Ordering::Relaxed);
        let token = started["token"].as_str().unwrap().to_owned();
        let options = StopOptions {
            expected_token: Some(token),
            ..StopOptions::default()
        };

        let first = manager
            .stop_with_options("worker", options.clone())
            .unwrap();
        let retained = fixture.client.panes.lock().unwrap().clone();
        let second = manager.stop_with_options("worker", options).unwrap();

        assert_eq!(second, first);
        assert_eq!(*fixture.client.panes.lock().unwrap(), retained);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn ordinary_muse_stop_retries_stopped_before_archive_without_retirement_receipt() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let token = started["token"].as_str().unwrap().to_owned();
        fixture.client.panes.lock().unwrap().clear();
        let mut record = manager.load("worker").unwrap();
        record.lifecycle = "stopped".to_owned();
        manager.save(&record).unwrap();
        let active = fixture.root.join("registry/worker");
        assert!(!active.join(MANAGED_DEAD_RETIREMENT_FILE).exists());

        let result = manager
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(token),
                    ..StopOptions::default()
                },
            )
            .unwrap();

        assert_eq!(result["pane_closed"], false);
        assert!(!active.exists());
        assert!(Path::new(result["archive"].as_str().unwrap()).is_dir());
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn ordinary_stopped_muse_never_closes_pane_created_after_absence_proof() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        let started = manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let token = started["token"].as_str().unwrap().to_owned();
        fixture.client.panes.lock().unwrap().clear();
        let mut record = manager.load("worker").unwrap();
        record.lifecycle = "stopped".to_owned();
        manager.save(&record).unwrap();
        fixture
            .client
            .replace_after_empty_panes
            .store(true, Ordering::Relaxed);

        let result = manager
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(token),
                    ..StopOptions::default()
                },
            )
            .unwrap();

        assert_eq!(result["ordinary_stop_recovered"], true);
        assert_eq!(result["pane_closed"], false);
        assert_eq!(
            fixture.client.panes.lock().unwrap()[0]
                .terminal_id
                .as_deref(),
            Some("replacement-after-absence")
        );
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn stopped_muse_without_retirement_receipt_never_closes_surviving_pane() {
        for replacement_terminal in [false, true] {
            let fixture = Fixture::new();
            let manager = fixture.manager();
            let started = manager
                .start(
                    "worker",
                    &fixture.root,
                    StartOptions {
                        workspace_id: Some("workspace".to_owned()),
                        harness: "muse".to_owned(),
                        ..StartOptions::default()
                    },
                )
                .unwrap();
            fixture.client.custom_alive.store(false, Ordering::Relaxed);
            let token = started["token"].as_str().unwrap().to_owned();
            let mut record = manager.load("worker").unwrap();
            record.lifecycle = "stopped".to_owned();
            manager.save(&record).unwrap();
            if replacement_terminal {
                fixture.client.panes.lock().unwrap()[0].terminal_id =
                    Some("replacement-terminal".to_owned());
            }
            let retained = fixture.client.panes.lock().unwrap().clone();

            let error = manager
                .stop_with_options(
                    "worker",
                    StopOptions {
                        expected_token: Some(token),
                        ..StopOptions::default()
                    },
                )
                .unwrap_err();

            assert!(error
                .to_string()
                .contains("no managed-dead retirement receipt"));
            assert_eq!(*fixture.client.panes.lock().unwrap(), retained);
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.root.join("registry/worker").is_dir());
        }
    }

    #[test]
    fn running_owned_dead_muse_archive_receipt_pins_one_directory_generation() {
        for swap_after_receipt in [false, true] {
            let fixture = Fixture::new();
            fixture
                .client
                .report_session
                .store(false, Ordering::Relaxed);
            let manager = fixture.manager();
            let started = manager
                .start(
                    "worker",
                    &fixture.root,
                    StartOptions {
                        workspace_id: Some("workspace".to_owned()),
                        harness: "muse".to_owned(),
                        ..StartOptions::default()
                    },
                )
                .unwrap();
            fixture.client.custom_alive.store(false, Ordering::Relaxed);
            let token = started["token"].as_str().unwrap().to_owned();
            let first = manager
                .stop_with_options(
                    "worker",
                    StopOptions {
                        expected_token: Some(token.clone()),
                        ..StopOptions::default()
                    },
                )
                .unwrap();
            let destination = PathBuf::from(first["archive"].as_str().unwrap());
            let replacement = destination.with_extension("replacement");
            let displaced = destination.with_extension("displaced");
            agent::create_private_directory(
                &replacement,
                "replacement managed-dead archive",
                true,
                true,
            )
            .unwrap();
            for name in ["agent.json", MANAGED_DEAD_RETIREMENT_FILE] {
                fs::copy(destination.join(name), replacement.join(name)).unwrap();
            }
            let replace = || {
                fs::rename(&destination, &displaced).unwrap();
                fs::rename(&replacement, &destination).unwrap();
            };
            let retained = fixture.client.panes.lock().unwrap().clone();

            let result = if swap_after_receipt {
                manager.managed_dead_archive_receipt_with("worker", Some(&token), || {}, replace)
            } else {
                manager.managed_dead_archive_receipt_with("worker", Some(&token), replace, || {})
            };

            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("directory changed"),
                "archive replacement after receipt={swap_after_receipt} was accepted"
            );
            assert_eq!(*fixture.client.panes.lock().unwrap(), retained);
            assert!(fixture.client.closed.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn custom_process_identity_record_is_atomic_and_strict() {
        let fixture = Fixture::new();
        let manager = fixture.manager();
        manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut record = agent::read_private_json(&path).unwrap();
        record["custom_process_identity"]
            .as_object_mut()
            .unwrap()
            .remove("starttime_ticks");
        agent::atomic_json(&path, &record).unwrap();
        let error = manager.load("worker").unwrap_err();
        assert!(error.to_string().contains("invalid agent record"));
    }

    #[test]
    fn foreign_shell_identity_record_is_strict_but_legacy_absence_loads() {
        let fixture = Fixture::new();
        fixture.adopt();
        let manager = fixture.manager();
        let path = fixture.root.join("registry/foreign/agent.json");
        let original = agent::read_private_json(&path).unwrap();

        let mut malformed = original.clone();
        malformed["foreign_shell_identity"]
            .as_object_mut()
            .unwrap()
            .remove("starttime_ticks");
        agent::atomic_json(&path, &malformed).unwrap();
        assert!(manager
            .load("foreign")
            .unwrap_err()
            .to_string()
            .contains("invalid agent record"));

        let mut wrong_adapter = original.clone();
        wrong_adapter["launch"]["adapter"] = json!("herdr");
        agent::atomic_json(&path, &wrong_adapter).unwrap();
        assert!(manager
            .load("foreign")
            .unwrap_err()
            .to_string()
            .contains("invalid agent record"));

        agent::atomic_json(&path, &original).unwrap();
        let mut legacy = manager.load("foreign").unwrap().public_value();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("foreign_shell_identity");
        agent::atomic_json(&path, &legacy).unwrap();
        assert!(manager
            .load("foreign")
            .unwrap()
            .foreign_shell_identity
            .is_none());
    }

    #[test]
    fn failed_muse_launch_cleanup_refuses_a_replacement_report() {
        let fixture = Fixture::new();
        fixture
            .client
            .custom_dies_after_report
            .store(true, Ordering::Relaxed);
        let manager = fixture.manager();
        assert!(manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .is_err());
        fixture
            .client
            .custom_reported
            .store(false, Ordering::Relaxed);
        let error = manager.stop("worker").unwrap_err();
        assert!(error.to_string().contains("foreground process"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn failed_muse_launch_cleanup_refuses_non_shell_foreground_process() {
        let fixture = Fixture::new();
        fixture
            .client
            .custom_dies_after_report
            .store(true, Ordering::Relaxed);
        let manager = fixture.manager();
        assert!(manager
            .start(
                "worker",
                &fixture.root,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    harness: "muse".to_owned(),
                    ..StartOptions::default()
                },
            )
            .is_err());
        fixture
            .client
            .custom_at_idle_shell
            .store(false, Ordering::Relaxed);
        let error = manager.stop("worker").unwrap_err();
        assert!(error.to_string().contains("foreground process"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }

    #[test]
    fn an_old_partial_allocation_can_retire_only_its_unclaimed_shell() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let mut record = manager.load("worker").unwrap();
        record.pane_id = None;
        record.lifecycle = "launch_failed".to_owned();
        manager.save(&record).unwrap();
        fixture.client.started.store(false, Ordering::Relaxed);
        let result = manager.stop("worker").unwrap();
        assert_eq!(result["pane_closed"], true);
        assert_eq!(*fixture.client.closed.lock().unwrap(), ["owned"]);
    }

    #[test]
    fn stop_preserves_a_human_pane_created_after_the_membership_check() {
        let fixture = Fixture::new();
        fixture.start(None);
        fixture
            .client
            .add_sibling_on_read
            .store(true, Ordering::Relaxed);
        let result = fixture.manager().stop("worker").unwrap();
        assert_eq!(result["pane_closed"], true);
        assert_eq!(result["tab_closed"], false);
        assert_eq!(*fixture.client.panes.lock().unwrap(), [Fake::pane("human")]);
        assert_eq!(*fixture.client.closed.lock().unwrap(), ["owned"]);
    }

    #[test]
    fn managed_dead_stop_requires_token_then_closes_exact_pane_and_archives() {
        let fixture = Fixture::new();
        let (pane, token) = fixture.make_managed_dead();
        let error = fixture.manager().stop("worker").unwrap_err();
        assert!(error.to_string().contains("requires --expected-token"));

        let stopped = fixture
            .manager()
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(token),
                    ..StopOptions::default()
                },
            )
            .unwrap();

        assert_eq!(stopped["managed_dead"], true);
        assert_eq!(stopped["pane_closed"], true);
        assert_eq!(stopped["tab_closed"], true);
        assert_eq!(*fixture.client.closed.lock().unwrap(), [pane]);
        let archive = PathBuf::from(stopped["archive"].as_str().unwrap());
        assert_eq!(
            agent::read_private_json(&archive.join("agent.json")).unwrap()["lifecycle"],
            "stopped"
        );
        assert!(archive.join("output.json").is_file());
    }

    #[test]
    fn managed_dead_stop_refuses_expanded_stopped_record_before_close() {
        let fixture = Fixture::new();
        let (_pane, token) = fixture.make_managed_dead();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut document = agent::read_private_json(&path).unwrap();
        document["extensions"]["future_padding"] = json!(vec!["x"; 150_000]);
        let mut raw = serde_json::to_vec(&document).unwrap();
        raw.push(b'\n');
        let mut stopped = document.clone();
        stopped["lifecycle"] = json!("stopped");
        let mut stopped_bytes = serde_json::to_vec_pretty(&stopped).unwrap();
        stopped_bytes.push(b'\n');
        assert!(raw.len() < MAX_AGENT_RECORD_BYTES);
        assert!(stopped_bytes.len() > MAX_AGENT_RECORD_BYTES);
        fs::write(&path, &raw).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let error = fixture
            .manager()
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(token.clone()),
                    ..StopOptions::default()
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("stopped agent record larger"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(fs::read(&path).unwrap(), raw);
        assert!(!fixture.root.join("registry/worker/output.json").exists());
        assert!(!fixture
            .root
            .join(format!("registry/archive/worker-{token}"))
            .exists());
    }

    #[test]
    fn managed_dead_preparation_never_restores_over_replaced_artifact_generations() {
        for phase in ["before-rename", "after-rename"] {
            let fixture = Fixture::new();
            let (_pane, token) = fixture.make_managed_dead();
            let manager = fixture.manager();
            let record = manager.load("worker").unwrap();
            let pinned = manager.pinned_agent_directory("worker").unwrap();
            let active = fixture.root.join("registry/worker");
            let prior = b"{\"prior\":true}\n";
            fs::write(active.join("output.json"), prior).unwrap();
            fs::set_permissions(
                active.join("output.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            let held = active.join(format!(".held-managed-{phase}"));

            let error = manager
                .retire_managed_dead_locked_with(
                    &record,
                    &pinned,
                    Some(&token),
                    || {},
                    || {},
                    (
                        |_pinned, temporary| {
                            if phase == "before-rename" {
                                fs::rename(active.join(temporary), &held).unwrap();
                                fs::write(active.join(temporary), b"replacement owner").unwrap();
                                fs::set_permissions(
                                    active.join(temporary),
                                    fs::Permissions::from_mode(0o600),
                                )
                                .unwrap();
                            }
                            Ok(())
                        },
                        |_pinned, name, temporary| {
                            if phase == "after-rename" {
                                fs::rename(active.join(name), &held).unwrap();
                                fs::write(active.join(name), b"replacement owner").unwrap();
                                fs::set_permissions(
                                    active.join(name),
                                    fs::Permissions::from_mode(0o600),
                                )
                                .unwrap();
                                fs::write(active.join(temporary), b"new temp owner").unwrap();
                                fs::set_permissions(
                                    active.join(temporary),
                                    fs::Permissions::from_mode(0o600),
                                )
                                .unwrap();
                            }
                            Ok(())
                        },
                    ),
                )
                .unwrap_err();

            assert!(
                error.to_string().contains("generation changed")
                    || error.to_string().contains("replacement was preserved")
            );
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(active.is_dir());
            assert!(fs::read_dir(fixture.root.join("registry/archive"))
                .unwrap()
                .next()
                .is_none());
            if phase == "before-rename" {
                assert_eq!(fs::read(active.join("output.json")).unwrap(), prior);
            } else {
                assert_eq!(
                    fs::read(active.join("output.json")).unwrap(),
                    b"replacement owner"
                );
                assert!(held.is_file());
            }
        }
    }

    #[test]
    fn managed_dead_stop_refuses_stopping_record_without_prior_proof() {
        let fixture = Fixture::new();
        let (_pane, token) = fixture.make_managed_dead();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut document = agent::read_private_json(&path).unwrap();
        document["lifecycle"] = json!("stopping");
        agent::atomic_json(&path, &document).unwrap();

        let error = fixture
            .manager()
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(token),
                    ..StopOptions::default()
                },
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("requires a running herdr record"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert!(path.is_file());
    }

    #[test]
    fn managed_dead_stop_refuses_crossed_mode_or_backend() {
        for (field, value) in [("mode", "headless"), ("backend", "tmux")] {
            let fixture = Fixture::new();
            let (_pane, token) = fixture.make_managed_dead();
            let path = fixture.root.join("registry/worker/agent.json");
            let mut document = agent::read_private_json(&path).unwrap();
            document[field] = json!(value);
            agent::atomic_json(&path, &document).unwrap();

            assert!(fixture
                .manager()
                .stop_with_options(
                    "worker",
                    StopOptions {
                        expected_token: Some(token),
                        ..StopOptions::default()
                    },
                )
                .is_err());
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(path.is_file());
        }
    }

    #[test]
    fn managed_dead_stop_refuses_runtime_or_registry_changes_without_closing() {
        for case in [
            "replacement",
            "session",
            "moved",
            "sibling",
            "shell",
            "record",
            "directory",
            "capture",
        ] {
            let fixture = Fixture::new();
            let (_pane, token) = fixture.make_managed_dead();
            match case {
                "replacement" => fixture
                    .client
                    .restart_foreign_on_read
                    .store(true, Ordering::Relaxed),
                "session" => fixture.client.report_session.store(true, Ordering::Relaxed),
                "moved" => fixture.client.panes.lock().unwrap()[0].tab_id = "moved".to_owned(),
                "sibling" => fixture
                    .client
                    .panes
                    .lock()
                    .unwrap()
                    .push(Fake::pane("human")),
                "shell" => fixture
                    .client
                    .change_foreign_shell_on_read
                    .store(true, Ordering::Relaxed),
                "record" => fixture
                    .client
                    .change_record_token_on_read
                    .store(true, Ordering::Relaxed),
                "directory" => fixture
                    .client
                    .replace_record_directory_on_read
                    .store(true, Ordering::Relaxed),
                "capture" => fixture.client.fail_read.store(true, Ordering::Relaxed),
                _ => unreachable!(),
            }
            assert!(fixture
                .manager()
                .stop_with_options(
                    "worker",
                    StopOptions {
                        expected_token: Some(token),
                        ..StopOptions::default()
                    },
                )
                .is_err());
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.root.join("registry/worker").is_dir());
        }
    }

    #[test]
    fn managed_dead_stop_reproves_runtime_after_artifact_preparation() {
        let fixture = Fixture::new();
        let (_pane, token) = fixture.make_managed_dead();
        let manager = fixture.manager();
        let record = manager.load("worker").unwrap();
        let pinned = manager.pinned_agent_directory("worker").unwrap();
        let record_path = fixture.root.join("registry/worker/agent.json");
        let original_record = fs::read(&record_path).unwrap();

        let error = manager
            .retire_managed_dead_locked_with(
                &record,
                &pinned,
                Some(&token),
                || {
                    fixture
                        .client
                        .foreign_shell_identity
                        .lock()
                        .unwrap()
                        .starttime_ticks += 1;
                },
                || {},
                (
                    |_pinned, _temporary| Ok(()),
                    |_pinned, _name, _temporary| Ok(()),
                ),
            )
            .unwrap_err();

        assert!(error.to_string().contains("immediately before close"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert_eq!(fs::read(record_path).unwrap(), original_record);
        assert!(!fixture.root.join("registry/worker/output.json").exists());
        assert!(fs::read_dir(fixture.root.join("registry/archive"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn managed_dead_stop_rechecks_registry_after_artifact_preparation() {
        for case in ["record", "directory", "output"] {
            let fixture = Fixture::new();
            let (_pane, token) = fixture.make_managed_dead();
            let manager = fixture.manager();
            let record = manager.load("worker").unwrap();
            let pinned = manager.pinned_agent_directory("worker").unwrap();
            let active = fixture.root.join("registry/worker");
            let record_path = active.join("agent.json");
            let original_record = fs::read(&record_path).unwrap();

            let error = manager
                .retire_managed_dead_locked_with(
                    &record,
                    &pinned,
                    Some(&token),
                    || {
                        if case == "record" {
                            let mut document = agent::read_private_json(&record_path).unwrap();
                            document["goal"] = json!("replacement record");
                            agent::atomic_json(&record_path, &document).unwrap();
                        } else if case == "directory" {
                            let displaced = fixture.root.join("registry/.worker-original");
                            fs::rename(&active, &displaced).unwrap();
                            fs::create_dir(&active).unwrap();
                            fs::set_permissions(&active, fs::Permissions::from_mode(0o700))
                                .unwrap();
                            fs::copy(displaced.join("agent.json"), active.join("agent.json"))
                                .unwrap();
                            fs::set_permissions(
                                active.join("agent.json"),
                                fs::Permissions::from_mode(0o600),
                            )
                            .unwrap();
                        } else {
                            let output = active.join("output.json");
                            fs::rename(&output, active.join(".prepared-output")).unwrap();
                            fs::write(&output, b"replacement output").unwrap();
                            fs::set_permissions(&output, fs::Permissions::from_mode(0o600))
                                .unwrap();
                        }
                    },
                    || {},
                    (
                        |_pinned, _temporary| Ok(()),
                        |_pinned, _name, _temporary| Ok(()),
                    ),
                )
                .unwrap_err();

            assert!(error.to_string().contains("changed"));
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fs::read_dir(fixture.root.join("registry/archive"))
                .unwrap()
                .next()
                .is_none());
            if case == "record" {
                assert_eq!(
                    agent::read_private_json(&record_path).unwrap()["goal"],
                    "replacement record"
                );
                assert_ne!(fs::read(record_path).unwrap(), original_record);
                assert!(!active.join("output.json").exists());
            } else if case == "directory" {
                let displaced = fixture.root.join("registry/.worker-original");
                assert_eq!(
                    fs::read(displaced.join("agent.json")).unwrap(),
                    original_record
                );
                assert!(!displaced.join("output.json").exists());
                assert!(active.is_dir());
            } else {
                assert_eq!(fs::read(&record_path).unwrap(), original_record);
                assert_eq!(
                    fs::read(active.join("output.json")).unwrap(),
                    b"replacement output"
                );
            }
        }
    }

    #[test]
    fn managed_dead_stop_rechecks_pane_after_final_record_proof() {
        for case in ["sibling", "agent"] {
            let fixture = Fixture::new();
            let (_pane, token) = fixture.make_managed_dead();
            let manager = fixture.manager();
            let record = manager.load("worker").unwrap();
            let pinned = manager.pinned_agent_directory("worker").unwrap();
            let active = fixture.root.join("registry/worker");

            let error = manager
                .retire_managed_dead_locked_with(
                    &record,
                    &pinned,
                    Some(&token),
                    || {},
                    || {
                        if case == "sibling" {
                            fixture
                                .client
                                .panes
                                .lock()
                                .unwrap()
                                .push(Fake::pane("human"));
                        } else {
                            fixture.client.started.store(true, Ordering::Relaxed);
                        }
                    },
                    (
                        |_pinned, _temporary| Ok(()),
                        |_pinned, _name, _temporary| Ok(()),
                    ),
                )
                .unwrap_err();

            let expected = if case == "sibling" {
                "recorded tab is not the exact one-pane tab"
            } else {
                "pane still reports agent"
            };
            assert!(error.to_string().contains(expected));
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(active.is_dir());
            assert!(!active.join("output.json").exists());
            assert!(fs::read_dir(fixture.root.join("registry/archive"))
                .unwrap()
                .next()
                .is_none());
        }
    }

    #[test]
    fn managed_dead_stop_rechecks_pane_after_final_shell_proof() {
        for mutation in [1, 2] {
            let fixture = Fixture::new();
            let (_pane, token) = fixture.make_managed_dead();
            fixture
                .client
                .shell_proof_mutation
                .store(mutation, Ordering::Relaxed);

            let error = fixture
                .manager()
                .stop_with_options(
                    "worker",
                    StopOptions {
                        expected_token: Some(token),
                        ..StopOptions::default()
                    },
                )
                .unwrap_err();

            let expected = if mutation == 1 {
                "recorded tab is not the exact one-pane tab"
            } else {
                "pane still reports agent"
            };
            assert!(error.to_string().contains(expected));
            assert!(fixture.client.closed.lock().unwrap().is_empty());
            assert!(fixture.root.join("registry/worker").is_dir());
            assert!(!fixture.root.join("registry/worker/output.json").exists());
            assert!(fs::read_dir(fixture.root.join("registry/archive"))
                .unwrap()
                .next()
                .is_none());
        }
    }

    #[test]
    fn managed_dead_stop_retries_after_close_result_is_uncertain() {
        let fixture = Fixture::new();
        let (_pane, token) = fixture.make_managed_dead();
        fixture
            .client
            .fail_after_close
            .store(true, Ordering::Relaxed);

        let error = fixture
            .manager()
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(token.clone()),
                    ..StopOptions::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("result loss after close"));
        assert_eq!(
            agent::read_private_json(&fixture.root.join("registry/worker/agent.json")).unwrap()
                ["lifecycle"],
            "running"
        );
        assert!(fixture.client.panes.lock().unwrap().is_empty());

        let stopped = fixture
            .manager()
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(token),
                    ..StopOptions::default()
                },
            )
            .unwrap();
        let archive = PathBuf::from(stopped["archive"].as_str().unwrap());
        assert!(!fixture.root.join("registry/worker").exists());
        assert_eq!(
            agent::read_private_json(&archive.join("agent.json")).unwrap()["lifecycle"],
            "stopped"
        );
    }

    #[test]
    fn managed_dead_stop_preflights_archive_collision_before_close() {
        let fixture = Fixture::new();
        let (_pane, token) = fixture.make_managed_dead();
        let destination = fixture
            .root
            .join(format!("registry/archive/worker-{token}"));
        fs::create_dir_all(&destination).unwrap();
        fs::set_permissions(
            destination.parent().unwrap(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();

        assert!(fixture
            .manager()
            .stop_with_options(
                "worker",
                StopOptions {
                    expected_token: Some(token),
                    ..StopOptions::default()
                },
            )
            .is_err());
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        assert!(fixture.root.join("registry/worker").is_dir());
    }

    #[test]
    fn paused_input_is_refused_but_human_attach_and_read_remain_available() {
        let fixture = Fixture::new();
        fixture.start(None);
        let manager = fixture.manager();
        let path = fixture.root.join("registry/worker/agent.json");
        let mut document = agent::read_private_json(&path).unwrap();
        document["extensions"]["future_metadata"] = json!({"nested":true});
        agent::atomic_json(&path, &document).unwrap();
        assert_eq!(manager.pause("worker", true).unwrap()["paused"], true);
        assert!(manager
            .send("worker", "task", DrainOptions::default())
            .is_err());
        assert!(manager.drain("worker", DrainOptions::default()).is_err());
        assert!(manager
            .goal("worker", Some("objective"), DrainOptions::default(), None)
            .is_err());
        assert_eq!(manager.attach("worker").unwrap()["paused"], true);
        assert_eq!(manager.read("worker", 10).unwrap(), "visible output");
        assert_eq!(
            manager.get("worker").unwrap()["future_metadata"],
            json!({"nested":true})
        );
        manager.pause("worker", false).unwrap();
        manager
            .send_identified("worker", "task", DrainOptions::default(), Some("request-1"))
            .unwrap();
        assert!(manager
            .send_identified("worker", "task", DrainOptions::default(), Some("request-1"))
            .is_err());
        assert_eq!(*fixture.client.runs.lock().unwrap(), ["task"]);
    }

    #[test]
    fn lost_native_status_event_reconciles_without_reinjecting_prompt() {
        let fixture = Fixture::new();
        fixture.start(None);
        fixture.client.wait_fails.store(true, Ordering::Relaxed);
        fixture
            .client
            .working_after_run
            .store(true, Ordering::Relaxed);

        let result = fixture
            .manager()
            .send(
                "worker",
                "review this exact change",
                DrainOptions::default(),
            )
            .unwrap();

        assert_eq!(result.outcome, agent::QueueOutcome::Delivered);
        assert_eq!(
            *fixture.client.runs.lock().unwrap(),
            ["review this exact change"]
        );
        assert_eq!(
            fs::read_dir(fixture.root.join("registry/worker/queue/processed"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(fixture.root.join("registry/worker/queue/failed"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn foreign_runtime_metadata_is_visible_without_herdr_mutation() {
        let fixture = Fixture::new();
        fixture.start(None);
        let path = fixture.root.join("registry/worker/agent.json");
        let mut document = agent::read_private_json(&path).unwrap();
        document["launch"]["adapter"] = json!("turn-runner");
        document["launch"]["mode"] = json!("headless");
        document["launch"]["runtime_home"] = json!("/tmp/worker-runtime");
        agent::atomic_json(&path, &document).unwrap();
        let manager = fixture.manager();
        let status = manager.status("worker").unwrap();
        assert_eq!(status["adapter"], "turn-runner");
        assert_eq!(status["agent_status"], "unknown");
        assert!(status["probe_error"]
            .as_str()
            .unwrap()
            .contains("worker extension"));
        assert_eq!(manager.list().unwrap().len(), 1);
        let health = manager.health(&["worker".to_owned()]);
        assert_eq!(health["healthy"], false);
        assert_eq!(health["sessions"][0]["health"], "unknown");
        assert_eq!(
            health["sessions"][0]["reason_code"],
            "runtime-observer-unavailable"
        );
        let durable =
            agent::read_private_json(&fixture.root.join("registry/worker/health.json")).unwrap();
        assert_eq!(durable["health"], "unknown");
        assert!(durable.get("last_unhealthy_reason").is_none());
        assert!(manager.stop("worker").is_err());
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }
}
