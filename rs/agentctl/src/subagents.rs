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

use crate::agent::{self, AgentApi, AgentError, DrainOptions, QueueResult, Target};
use crate::client::{
    muse_idle_composer, muse_prompt_in_composer, muse_prompt_transcript_count,
    muse_startup_metadata, muse_trust_prompt, AgentPaneInfo, CustomProcessIdentity, HerdrClient,
    Pane, PaneShellProof,
};

const BRACKETED_PASTE_START: &str = "\u{1b}[200~";
const BRACKETED_PASTE_END: &str = "\u{1b}[201~";
const MAX_AGENT_RECORD_BYTES: usize = 1024 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

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
    if !matches!(name, "agent.json" | "output.json") {
        return Err(fail("unsupported pinned registry artifact name").into());
    }
    let limit = if name == "agent.json" {
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
        _persist_identity: &mut dyn FnMut(CustomProcessIdentity) -> crate::error::Result<()>,
    ) -> crate::error::Result<()> {
        Err(crate::error::AdapterError::unavailable(
            "custom pane harness launch is unavailable",
        ))
    }
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
        persist_identity: &mut dyn FnMut(CustomProcessIdentity) -> crate::error::Result<()>,
    ) -> crate::error::Result<()> {
        HerdrClient::start_pane_agent(self, name, harness, pane, args, timeout, persist_identity)
    }
    fn verify_custom_harness(
        &self,
        pane: &str,
        harness: &str,
        identity: Option<&CustomProcessIdentity>,
    ) -> crate::error::Result<()> {
        HerdrClient::verify_custom_harness(self, pane, harness, identity)
    }
    fn pane_is_idle_shell(&self, pane: &str) -> crate::error::Result<bool> {
        HerdrClient::pane_is_idle_shell(self, pane)
    }
    fn pane_shell_identity(&self, pane: &str) -> crate::error::Result<CustomProcessIdentity> {
        HerdrClient::pane_shell_identity(self, pane)
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
struct AgentRecord {
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
    model: Option<String>,
    resume: Option<String>,
    arguments: Vec<String>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeadPaneProof {
    info: AgentPaneInfo,
    presentation: Pane,
    shell: PaneShellProof,
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

impl AgentRecord {
    fn validate_loaded(&self, path: &Path, agent_name: &str) -> Result<()> {
        if self.name != agent_name
            || self.schema != 1
            || self.token.is_empty()
            || self.token.len() > 80
            || !self
                .token
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || self.harness.is_empty()
            || self.cwd.is_empty()
            || self.lifecycle.is_empty()
            || !self.created_at.is_finite()
            || self
                .goal_message_id
                .as_deref()
                .is_some_and(|value| !message_id(value))
            || self
                .goal_messages
                .iter()
                .any(|(key, value)| !message_id(key) || value.is_empty())
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
                    self.adapter != "herdr-pane"
                        || self.harness != "muse"
                        || self.pane_id.as_deref().is_none_or(str::is_empty)
                        || !identity.valid()
                })
            || self
                .foreign_shell_identity
                .as_ref()
                .is_some_and(|identity| {
                    self.adapter != "herdr-foreign"
                        || self.pane_id.as_deref().is_none_or(str::is_empty)
                        || !identity.valid()
                })
        {
            return Err(fail(format!("invalid agent record: {}", path.display())));
        }
        Ok(())
    }

    fn supported(&self) -> Result<()> {
        if !matches!(
            self.adapter.as_str(),
            "herdr" | "herdr-pane" | "herdr-foreign"
        ) || self.mode != "interactive"
            || self.backend != "herdr"
        {
            return Err(fail(format!("agent {:?} uses adapter {:?}, mode {:?}, backend {:?}; use the agentctl with the worker extension implementation for this runtime", self.name, self.adapter, self.mode, self.backend)));
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
        Ok(Target {
            pane_id: self.pane_id.clone(),
            session_agent: self.session_agent.clone(),
            session_value: self.session_value.clone(),
            expected_agent: Some(self.harness.clone()),
            expected_cwd: Some(PathBuf::from(&self.cwd)),
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

#[derive(Clone, Debug)]
struct CustomPaneSubmission {
    staged_screen: String,
    text: String,
    prior_transcript_count: usize,
}

struct WorkspaceClient<'a, A: ManagedApi + ?Sized> {
    client: &'a A,
    record: &'a AgentRecord,
    goal_objective: Mutex<Option<String>>,
    custom_submission: Mutex<Option<CustomPaneSubmission>>,
    queue: Option<&'a Path>,
    check_prompt: bool,
}

impl<A: ManagedApi + ?Sized> AgentApi for WorkspaceClient<'_, A> {
    fn panes(&self) -> crate::error::Result<Vec<Pane>> {
        Ok(self
            .client
            .panes()?
            .into_iter()
            .filter(|pane| Some(&pane.workspace_id) == self.record.workspace_id.as_ref())
            .collect())
    }
    fn pane_info(&self, pane_id: &str) -> crate::error::Result<AgentPaneInfo> {
        if self.record.adapter == "herdr"
            && Some(self.client.agent_pane(&self.record.name)?) != self.record.pane_id
        {
            return Err(crate::error::AdapterError::unavailable(format!(
                "agent {:?} no longer owns its recorded pane",
                self.record.name
            )));
        }
        let mut info = self.client.pane_info(pane_id)?;
        if Some(&info.workspace_id) != self.record.workspace_id.as_ref() {
            return Err(crate::error::AdapterError::unavailable(format!(
                "agent {:?} workspace identity changed",
                self.record.name
            )));
        }
        if Some(pane_id) == self.record.pane_id.as_deref() {
            if self.record.goal_session_id.is_some()
                && info.session_value.is_some()
                && info.session_value != self.record.goal_session_id
            {
                return Err(crate::error::AdapterError::unavailable(format!(
                    "agent {:?} native session identity changed",
                    self.record.name
                )));
            }
            if self.check_prompt
                && matches!(info.status.as_str(), "idle" | "done")
                && info.agent.as_deref() == Some("claude")
            {
                let screen = self.client.read(pane_id, "visible", Some(200))?;
                if screen
                    .contains("Quick safety check: Is this a project you created or one you trust?")
                    && screen.contains("No, exit")
                    && screen.contains("Yes, I trust this folder")
                {
                    return Err(crate::error::AdapterError::unavailable("Claude workspace trust prompt requires human attention; no input was submitted"));
                }
            }
            if self.record.adapter == "herdr-pane" {
                self.client.verify_custom_harness(
                    pane_id,
                    &self.record.harness,
                    self.record.custom_process_identity.as_ref(),
                )?;
                let screen = self.client.read(pane_id, "visible", Some(200))?;
                if muse_trust_prompt(&screen) {
                    return Err(crate::error::AdapterError::unavailable(
                        "Muse workspace trust prompt requires human attention; no input was submitted",
                    ));
                }
                info.status = if muse_idle_composer(&screen) {
                    "idle".to_owned()
                } else {
                    "working".to_owned()
                };
            }
        }
        Ok(info)
    }
    fn workspace_label(&self, workspace_id: &str) -> crate::error::Result<String> {
        self.client.workspace_label(workspace_id)
    }
    fn run(&self, pane_id: &str, text: &str) -> crate::error::Result<()> {
        *self
            .goal_objective
            .lock()
            .expect("goal operation lock poisoned") = None;
        if let Some(queue) = self.queue {
            for (identifier, objective) in &self.record.goal_messages {
                let path = queue.join("inflight").join(format!("{identifier}.json"));
                if text == format!("/goal {objective}") && fs::symlink_metadata(&path).is_ok() {
                    let document = agent::read_private_json(&path).map_err(|error| {
                        crate::error::AdapterError::unavailable(error.to_string())
                    })?;
                    if document["text"].as_str() == Some(text) {
                        *self
                            .goal_objective
                            .lock()
                            .expect("goal operation lock poisoned") = Some(objective.clone());
                        break;
                    }
                }
            }
        }
        if self.record.adapter != "herdr-pane" {
            return self.client.run(pane_id, text);
        }
        if text.contains(['\0', '\u{1b}']) {
            return Err(crate::error::AdapterError::unavailable(
                "Muse pane prompts cannot contain NUL or terminal escape characters",
            ));
        }
        let info = self.pane_info(pane_id)?;
        if info.status != "idle" {
            return Err(crate::error::AdapterError::unavailable(format!(
                "custom pane {pane_id} is not at a verified idle Muse composer"
            )));
        }
        let before = self.client.read(pane_id, "visible", Some(200))?;
        self.client.send_text(
            pane_id,
            &format!("{BRACKETED_PASTE_START}{text}{BRACKETED_PASTE_END}"),
        )?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let staged = loop {
            self.client.verify_custom_harness(
                pane_id,
                &self.record.harness,
                self.record.custom_process_identity.as_ref(),
            )?;
            let screen = self.client.read(pane_id, "visible", Some(200))?;
            if screen != before && muse_prompt_in_composer(&screen, text) {
                break screen;
            }
            if Instant::now() >= deadline {
                return Err(crate::error::AdapterError::unavailable(
                    "literal text insertion did not produce exact visible Muse editor evidence; Enter was not sent",
                ));
            }
            std::thread::sleep(
                Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
            );
        };
        self.client.verify_custom_harness(
            pane_id,
            &self.record.harness,
            self.record.custom_process_identity.as_ref(),
        )?;
        self.client.send_keys(pane_id, "Enter")?;
        *self
            .custom_submission
            .lock()
            .expect("custom submission lock poisoned") = Some(CustomPaneSubmission {
            prior_transcript_count: muse_prompt_transcript_count(&staged, text),
            staged_screen: staged,
            text: text.to_owned(),
        });
        Ok(())
    }
    fn wait_agent_status(
        &self,
        pane_id: &str,
        status: &str,
        timeout_ms: u64,
    ) -> crate::error::Result<()> {
        if self.record.adapter == "herdr-pane" && status == "working" {
            let submission = self
                .custom_submission
                .lock()
                .expect("custom submission lock poisoned")
                .clone()
                .ok_or_else(|| {
                    crate::error::AdapterError::unavailable(
                        "custom pane harness has no pending submission receipt",
                    )
                })?;
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            loop {
                self.client.verify_custom_harness(
                    pane_id,
                    &self.record.harness,
                    self.record.custom_process_identity.as_ref(),
                )?;
                let screen = self.client.read(pane_id, "visible", Some(200))?;
                if screen != submission.staged_screen
                    && muse_prompt_transcript_count(&screen, &submission.text)
                        > submission.prior_transcript_count
                    && !muse_prompt_in_composer(&screen, &submission.text)
                {
                    *self
                        .custom_submission
                        .lock()
                        .expect("custom submission lock poisoned") = None;
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(crate::error::AdapterError::unavailable(
                        "Muse did not show a verified post-Enter screen transition",
                    ));
                }
                std::thread::sleep(
                    Duration::from_millis(50)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
        let Some(objective) = self
            .goal_objective
            .lock()
            .expect("goal operation lock poisoned")
            .clone()
            .filter(|_| status == "working")
        else {
            return self.client.wait_agent_status(pane_id, status, timeout_ms);
        };
        let start = Instant::now();
        if self
            .client
            .wait_agent_status(pane_id, status, timeout_ms.min(1000))
            .is_ok()
        {
            return Ok(());
        }
        self.pane_info(pane_id)?;
        let screen = self.client.read(pane_id, "visible", Some(200))?;
        if goal_replacement_selected(&screen, &objective) {
            self.client.send_keys(pane_id, "Enter")?;
        }
        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.client
            .wait_agent_status(pane_id, status, timeout_ms.saturating_sub(elapsed).max(1))
    }
    fn read(
        &self,
        pane_id: &str,
        source: &str,
        lines: Option<usize>,
    ) -> crate::error::Result<String> {
        self.client.read(pane_id, source, lines)
    }

    fn panes_with_runtime(
        &self,
        runtime: &dyn agent::AgentRuntime,
    ) -> crate::error::Result<Vec<Pane>> {
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
        if self.record.adapter == "herdr"
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
            if self.record.goal_session_id.is_some()
                && info.session_value.is_some()
                && info.session_value != self.record.goal_session_id
            {
                return Err(crate::error::AdapterError::unavailable(format!(
                    "agent {:?} native session identity changed",
                    self.record.name
                )));
            }
            if self.check_prompt
                && matches!(info.status.as_str(), "idle" | "done")
                && info.agent.as_deref() == Some("claude")
            {
                let screen =
                    self.client
                        .read_with_runtime(pane_id, "visible", Some(200), runtime)?;
                if screen
                    .contains("Quick safety check: Is this a project you created or one you trust?")
                    && screen.contains("No, exit")
                    && screen.contains("Yes, I trust this folder")
                {
                    return Err(crate::error::AdapterError::unavailable("Claude workspace trust prompt requires human attention; no input was submitted"));
                }
            }
            if self.record.adapter == "herdr-pane" {
                self.client.verify_custom_harness_with_runtime(
                    pane_id,
                    &self.record.harness,
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
                info.status = if muse_idle_composer(&screen) {
                    "idle".to_owned()
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
        if let Some(queue) = self.queue {
            for (identifier, objective) in &self.record.goal_messages {
                let path = queue.join("inflight").join(format!("{identifier}.json"));
                if text == format!("/goal {objective}") && fs::symlink_metadata(&path).is_ok() {
                    let document = agent::read_private_json(&path).map_err(|error| {
                        crate::error::AdapterError::unavailable(error.to_string())
                    })?;
                    if document["text"].as_str() == Some(text) {
                        *self
                            .goal_objective
                            .lock()
                            .expect("goal operation lock poisoned") = Some(objective.clone());
                        break;
                    }
                }
            }
        }
        if self.record.adapter != "herdr-pane" {
            return self.client.run_with_runtime(pane_id, text, runtime);
        }
        if text.contains(['\0', '\u{1b}']) {
            return Err(crate::error::AdapterError::unavailable(
                "Muse pane prompts cannot contain NUL or terminal escape characters",
            ));
        }
        let info = self.pane_info_with_runtime(pane_id, runtime)?;
        if info.status != "idle" {
            return Err(crate::error::AdapterError::unavailable(format!(
                "custom pane {pane_id} is not at a verified idle Muse composer"
            )));
        }
        let before = self
            .client
            .read_with_runtime(pane_id, "visible", Some(200), runtime)?;
        self.client.send_text_with_runtime(
            pane_id,
            &format!("{BRACKETED_PASTE_START}{text}{BRACKETED_PASTE_END}"),
            runtime,
        )?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let staged = loop {
            if runtime.cancelled() {
                return Err(crate::error::AdapterError::unavailable(
                    "Herdr control operation was cancelled",
                ));
            }
            self.client.verify_custom_harness_with_runtime(
                pane_id,
                &self.record.harness,
                self.record.custom_process_identity.as_ref(),
                runtime,
            )?;
            let screen = self
                .client
                .read_with_runtime(pane_id, "visible", Some(200), runtime)?;
            if screen != before && muse_prompt_in_composer(&screen, text) {
                break screen;
            }
            if Instant::now() >= deadline {
                return Err(crate::error::AdapterError::unavailable(
                    "literal text insertion did not produce exact visible Muse editor evidence; Enter was not sent",
                ));
            }
            std::thread::sleep(
                Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
            );
        };
        self.client.verify_custom_harness_with_runtime(
            pane_id,
            &self.record.harness,
            self.record.custom_process_identity.as_ref(),
            runtime,
        )?;
        self.client
            .send_keys_with_runtime(pane_id, "Enter", runtime)?;
        *self
            .custom_submission
            .lock()
            .expect("custom submission lock poisoned") = Some(CustomPaneSubmission {
            prior_transcript_count: muse_prompt_transcript_count(&staged, text),
            staged_screen: staged,
            text: text.to_owned(),
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
        if self.record.adapter == "herdr-pane" && status == "working" {
            let submission = self
                .custom_submission
                .lock()
                .expect("custom submission lock poisoned")
                .clone()
                .ok_or_else(|| {
                    crate::error::AdapterError::unavailable(
                        "custom pane harness has no pending submission receipt",
                    )
                })?;
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            loop {
                if runtime.cancelled() {
                    return Err(crate::error::AdapterError::unavailable(
                        "Herdr control operation was cancelled",
                    ));
                }
                self.client.verify_custom_harness_with_runtime(
                    pane_id,
                    &self.record.harness,
                    self.record.custom_process_identity.as_ref(),
                    runtime,
                )?;
                let screen =
                    self.client
                        .read_with_runtime(pane_id, "visible", Some(200), runtime)?;
                if screen != submission.staged_screen
                    && muse_prompt_transcript_count(&screen, &submission.text)
                        > submission.prior_transcript_count
                    && !muse_prompt_in_composer(&screen, &submission.text)
                {
                    *self
                        .custom_submission
                        .lock()
                        .expect("custom submission lock poisoned") = None;
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(crate::error::AdapterError::unavailable(
                        "Muse did not show a verified post-Enter screen transition",
                    ));
                }
                std::thread::sleep(
                    Duration::from_millis(50)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
        let Some(objective) = self
            .goal_objective
            .lock()
            .expect("goal operation lock poisoned")
            .clone()
            .filter(|_| status == "working")
        else {
            return self
                .client
                .wait_agent_status_with_runtime(pane_id, status, timeout_ms, runtime);
        };
        let start = Instant::now();
        if self
            .client
            .wait_agent_status_with_runtime(pane_id, status, timeout_ms.min(1000), runtime)
            .is_ok()
        {
            return Ok(());
        }
        self.pane_info_with_runtime(pane_id, runtime)?;
        let screen = self
            .client
            .read_with_runtime(pane_id, "visible", Some(200), runtime)?;
        if goal_replacement_selected(&screen, &objective) {
            self.client
                .send_keys_with_runtime(pane_id, "Enter", runtime)?;
        }
        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.client.wait_agent_status_with_runtime(
            pane_id,
            status,
            timeout_ms.saturating_sub(elapsed).max(1),
            runtime,
        )
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
    /// Existing workspace ID, or the current workspace / shared `subagents` default.
    pub workspace_id: Option<String>,
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
            workspace_id: None,
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
        agent::validate_private_directory(&path, "agent directory", false)?;
        let before = fs::symlink_metadata(&path)
            .map_err(|error| fail(format!("cannot inspect agent directory: {error}")))?;
        let (device, inode) = Self::private_directory_identity(&before, "agent directory")?;
        after_preopen_check();
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|error| fail(format!("cannot pin agent directory: {error}")))?;
        let metadata = file
            .metadata()
            .map_err(|error| fail(format!("cannot inspect pinned agent directory: {error}")))?;
        let opened = Self::private_directory_identity(&metadata, "pinned agent directory")?;
        if opened != (device, inode) {
            return Err(fail("agent directory changed while being pinned"));
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
        if require_active_name {
            Self::verify_pinned_agent_directory(pinned)?;
        }
        let path = pinned.path.join("agent.json");
        let mut file = Self::open_pinned_file(pinned, "agent.json", libc::O_RDONLY)?;
        let before = file.metadata().map_err(|error| {
            fail(format!(
                "cannot inspect agent record {}: {error}",
                path.display()
            ))
        })?;
        let uid = unsafe { libc::getuid() };
        if !before.is_file()
            || before.uid() != uid
            || before.permissions().mode() & 0o077 != 0
            || before.nlink() != 1
            || before.len() > MAX_AGENT_RECORD_BYTES as u64
        {
            return Err(fail(format!(
                "unsafe agent record for recovery: {}",
                path.display()
            )));
        }
        let mut content = Vec::with_capacity(before.len() as usize);
        Read::by_ref(&mut file)
            .take((MAX_AGENT_RECORD_BYTES + 1) as u64)
            .read_to_end(&mut content)
            .map_err(|error| {
                fail(format!(
                    "cannot read agent record {}: {error}",
                    path.display()
                ))
            })?;
        let after = file.metadata().map_err(|error| {
            fail(format!(
                "cannot reinspect agent record {}: {error}",
                path.display()
            ))
        })?;
        if content.len() > MAX_AGENT_RECORD_BYTES
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
                "agent record changed while reading: {}",
                path.display()
            )));
        }
        if require_active_name {
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
        let record: AgentRecord = serde_json::from_value(agent::read_private_json(&path)?)
            .map_err(|error| fail(format!("invalid agent record {}: {error}", path.display())))?;
        record.validate_loaded(&path, agent_name)?;
        Ok(record)
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
        let document: Value = serde_json::from_slice(&content).map_err(|error| {
            fail(format!(
                "cannot inspect legacy agent record {}: {error}",
                path.display()
            ))
        })?;
        let object = document
            .as_object()
            .ok_or_else(|| fail(format!("invalid legacy agent record: {}", path.display())))?;
        if object.contains_key("foreign_shell_identity") {
            return Err(fail(
                "--recover-legacy-adoption requires foreign_shell_identity to be absent, not null or populated",
            ));
        }
        let record: AgentRecord = serde_json::from_value(document)
            .map_err(|error| fail(format!("invalid agent record {}: {error}", path.display())))?;
        record.validate_loaded(&path, &pinned.name)?;
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
        let record: AgentRecord = serde_json::from_slice(&content)
            .map_err(|error| fail(format!("invalid agent record {}: {error}", path.display())))?;
        record.validate_loaded(&path, &pinned.name)?;
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
        agent::atomic_json(
            &self.directory(&record.name)?.join("agent.json"),
            &json!(record),
        )
    }

    fn queue(&self, agent_name: &str) -> Result<PathBuf> {
        Ok(self.directory(agent_name)?.join("queue"))
    }

    /// Read durable metadata even if Herdr is unreachable.
    pub fn get(&self, agent_name: &str) -> Result<Value> {
        Ok(json!(self.load(agent_name)?))
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
            if other.session_agent.as_deref().unwrap_or(&other.harness) == session_agent
                && (other.session_value.as_deref() == Some(session_value)
                    || other.goal_session_id.as_deref() == Some(session_value))
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
            adapter: if options.harness == "muse" {
                "herdr-pane".to_owned()
            } else {
                herdr_adapter()
            },
            mode: interactive_mode(),
            backend: herdr_adapter(),
            paused: false,
            runtime_home: None,
            pane_reported_by_agentctl: false,
            custom_process_identity: None,
            foreign_shell_identity: None,
            extra: BTreeMap::new(),
            name: agent_name.to_owned(),
            token: format!("{}-{}", now.as_nanos(), std::process::id()),
            harness: options.harness.clone(),
            cwd: cwd.display().to_string(),
            created_at: now.as_secs_f64(),
            schema: 1,
            lifecycle: "starting".to_owned(),
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            session_agent: None,
            session_value: None,
            model: options.model.clone(),
            resume: options.resume.clone(),
            arguments,
            startup_warning: None,
            effective_reasoning_effort: None,
            error: None,
            goal: None,
            goal_delivery: None,
            goal_session_id: None,
            goal_command: None,
            goal_messages: BTreeMap::new(),
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
                && other.session_agent.as_deref().unwrap_or(&other.harness)
                    == info.session_agent.as_deref().unwrap_or("")
                && (other.session_value == info.session_value
                    || other.goal_session_id == info.session_value);
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
            adapter: "herdr-foreign".to_owned(),
            mode: interactive_mode(),
            backend: herdr_adapter(),
            paused: false,
            runtime_home: None,
            pane_reported_by_agentctl: false,
            custom_process_identity: None,
            foreign_shell_identity: Some(shell_identity.clone()),
            extra: BTreeMap::new(),
            name: agent_name.to_owned(),
            token: format!("{}-{}", now.as_nanos(), std::process::id()),
            harness: options.harness,
            cwd: cwd.display().to_string(),
            created_at: now.as_secs_f64(),
            schema: 1,
            lifecycle: "running".to_owned(),
            workspace_id: Some(info.workspace_id),
            tab_id: Some(presentation.tab_id.clone()),
            pane_id: Some(info.pane_id),
            session_agent: info.session_agent,
            session_value: info.session_value,
            model: None,
            resume: None,
            arguments: Vec::new(),
            startup_warning: None,
            effective_reasoning_effort: None,
            error: None,
            goal: None,
            goal_delivery: None,
            goal_session_id: None,
            goal_command: None,
            goal_messages: BTreeMap::new(),
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
        agent::atomic_json(&destination.join("agent.json"), &json!(record))?;
        Ok(destination)
    }

    fn launch(&self, record: &mut AgentRecord, options: &StartOptions) -> Result<()> {
        self.create_presentation(record, options)?;
        let pane_id = record.pane_id.clone().expect("new tab has pane");
        if record.adapter == "herdr-pane" {
            let agent_name = record.name.clone();
            let harness = record.harness.clone();
            let arguments = record.arguments.clone();
            self.client.start_pane_agent(
                &agent_name,
                &harness,
                &pane_id,
                &arguments,
                options.startup_timeout,
                &mut |identity| {
                    record.custom_process_identity = Some(identity);
                    self.save(record).map_err(|error| {
                        crate::error::AdapterError::unavailable(format!(
                            "cannot persist custom process identity: {error}"
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
                &record.harness,
                &pane_id,
                &record.arguments,
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
            },
            &record.target()?,
        )?;
        record.session_agent = info.session_agent;
        record.session_value = info.session_value;
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
                    return Err(fail(format!(
                        "native session is already registered as {:?}; closed the conflicting new pane",
                        owner.name
                    )));
                }
                Err(error) => {
                    record.session_agent = None;
                    record.session_value = None;
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
                return Err(error);
            }
        };
        if final_info.session_agent != record.session_agent
            || final_info.session_value != record.session_value
        {
            record.session_agent = None;
            record.session_value = None;
            return Err(fail(
                "started agent native session changed during identity commit",
            ));
        }
        Ok(())
    }

    fn create_presentation(&self, record: &mut AgentRecord, options: &StartOptions) -> Result<()> {
        let lock = agent::open_private_lock(
            &agent::target_lock_path("managed-workspace:subagents")?,
            "workspace allocation lock",
        )?;
        lock.lock_exclusive()
            .map_err(|error| fail(error.to_string()))?;
        let selected = options
            .workspace_id
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.inherited_workspace.clone());
        let selected = if let Some(workspace) = selected {
            self.client.workspace_label(&workspace)?;
            Some(workspace)
        } else {
            self.client.workspace_id_for_label("subagents")?
        };
        match selected {
            Some(workspace) => {
                record.workspace_id = Some(workspace.clone());
                let (tab, pane) = self.client.create_tab_with_pane(
                    &workspace,
                    &record.name,
                    &record.cwd,
                    &options.environment,
                )?;
                record.tab_id = Some(tab);
                record.pane_id = Some(pane);
                self.save(record)?;
            }
            None => {
                let (workspace, tab, pane) =
                    self.client
                        .create_workspace("subagents", &record.cwd, &options.environment)?;
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
            },
            &record.target()?,
        )
    }

    fn inspect_foreign(
        &self,
        agent_name: &str,
        record: &AgentRecord,
    ) -> Result<(bool, AgentPaneInfo, Pane)> {
        let pane_id = record
            .pane_id
            .as_deref()
            .ok_or_else(|| fail("adopted agent record has no pane identity"))?;
        let mut presentations: Vec<Pane> = self
            .client
            .panes()?
            .into_iter()
            .filter(|pane| pane.pane_id == pane_id)
            .collect();
        if presentations.len() != 1 {
            return Err(fail(format!(
                "refusing to unregister adopted agent {agent_name:?}: expected one recorded pane, found {}",
                presentations.len()
            )));
        }
        let presentation = presentations.pop().expect("one presentation was checked");
        if record.workspace_id.as_deref() != Some(presentation.workspace_id.as_str()) {
            return Err(fail(format!(
                "refusing to unregister adopted agent {agent_name:?}: recorded presentation workspace changed"
            )));
        }
        let info = self.client.pane_info(&presentation.pane_id)?;
        let cwd_matches = info.cwd == record.cwd
            || fs::canonicalize(&info.cwd)
                .ok()
                .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.cwd).ok());
        if info.pane_id != presentation.pane_id
            || Some(&info.workspace_id) != record.workspace_id.as_ref()
            || !cwd_matches
        {
            return Err(fail(format!(
                "refusing to unregister adopted agent {agent_name:?}: recorded pane, workspace, or cwd changed"
            )));
        }
        let shell_identity = record.foreign_shell_identity.as_ref().ok_or_else(|| {
            fail(format!(
                "refusing to unregister adopted agent {agent_name:?}: legacy record has no identity-bound pane shell"
            ))
        })?;
        if self.client.pane_shell_identity(&info.pane_id)? != *shell_identity {
            return Err(fail(format!(
                "refusing to unregister adopted agent {agent_name:?}: recorded pane shell generation changed"
            )));
        }
        if info.agent.is_none() {
            // A live agent is pinned by its harness and, when one was
            // reported, native session. A returned shell has no such process
            // identity, so its recorded tab remains part of the fallback
            // proof. This still lets an operator unregister a fully
            // revalidated live agent after moving its pane between tabs in the
            // same workspace.
            if record.tab_id.as_deref() != Some(presentation.tab_id.as_str()) {
                return Err(fail(format!(
                    "refusing to unregister adopted agent {agent_name:?}: recorded tab changed while the agent was absent"
                )));
            }
            if info.session_agent.is_some() || info.session_value.is_some() {
                return Err(fail(format!(
                    "refusing pane {}: absent agent has native session identity",
                    info.pane_id
                )));
            }
            if !self
                .client
                .pane_is_same_idle_shell(&info.pane_id, shell_identity)?
            {
                return Err(fail(format!(
                    "refusing pane {}: absent agent is not at the recorded identity-bound idle shell process group",
                    info.pane_id
                )));
            }
            return Ok((false, info, presentation));
        }
        Ok((true, self.checked(record)?, presentation))
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
            },
            &record.target()?,
            runtime,
        )
    }

    fn status_record(&self, record: &AgentRecord) -> Result<Value> {
        let agent_name = &record.name;
        let mut result = json!(record);
        result["queue"] = json!(self.queue(agent_name)?);
        result["output"] = json!(self.directory(agent_name)?.join("output.json"));
        result["goal_source"] = if record.goal.is_some() {
            json!("requested")
        } else {
            Value::Null
        };
        result["goal_delivery"] = json!(self.goal_delivery(record));
        let client = WorkspaceClient {
            client: self.client,
            record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: None,
            check_prompt: false,
        };
        let probe = record.target().and_then(|target| {
            agent::resolve_target(&client, &target)
                .and_then(|_| agent::status(&client, &target, &self.queue(agent_name)?))
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
        Ok(result)
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
        record.input_allowed()?;
        let queue = self.queue(&record.name)?;
        let client = WorkspaceClient {
            client: self.client,
            record: &record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: Some(&queue),
            check_prompt: true,
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
        let queue = self.queue(&record.name)?;
        let client = WorkspaceClient {
            client: self.client,
            record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: Some(&queue),
            check_prompt: true,
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
        record.supported()?;
        record.paused = paused;
        self.save(&record)?;
        Ok(json!({"name": agent_name, "token": record.token, "paused": paused}))
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
        record.input_allowed()?;
        agent::drain(
            &WorkspaceClient {
                client: self.client,
                record: &record,
                goal_objective: Mutex::new(None),
                custom_submission: Mutex::new(None),
                queue: Some(&self.queue(agent_name)?),
                check_prompt: true,
            },
            &record.target()?,
            &self.queue(agent_name)?,
            options,
        )
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
        let queue = self.queue(&record.name)?;
        agent::drain_with_runtime(
            &WorkspaceClient {
                client: self.client,
                record: &record,
                goal_objective: Mutex::new(None),
                custom_submission: Mutex::new(None),
                queue: Some(&queue),
                check_prompt: true,
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
            if matches!(info.status.as_str(), "idle" | "done") {
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
        let info = self.checked(&record)?;
        for existing in [
            &record.goal_session_id,
            &record.session_value,
            &info.session_value,
        ]
        .into_iter()
        .flatten()
        {
            if existing != session_id {
                return Err(fail("refusing to replace an already bound native session"));
            }
        }
        if let Some(owner) = self.identity_owner(&record.harness, session_id, Some(agent_name))? {
            return Err(fail(format!(
                "native session is already registered as {:?}",
                owner.name
            )));
        }
        let mut reported = Vec::new();
        for pane in self.client.panes()? {
            let live = self.client.pane_info(&pane.pane_id)?;
            if live.session_agent.as_deref() == Some(&record.harness)
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
        record.goal_session_id = Some(session_id.to_owned());
        if let Some(command) = goal_command {
            record.goal_command = Some(command.to_vec());
        }
        self.save(&record)?;
        Ok(json!({"name":agent_name,"session_id":session_id,"source":"explicit"}))
    }

    fn goal_delivery(&self, record: &AgentRecord) -> Option<String> {
        if let (Some(identifier), Ok(queue)) = (&record.goal_message_id, self.queue(&record.name)) {
            for (folder, outcome) in [
                ("processed", "delivered"),
                ("failed", "possibly_submitted"),
                ("inflight", "possibly_submitted"),
                ("inbox", "pending"),
            ] {
                if fs::symlink_metadata(queue.join(folder).join(format!("{identifier}.json")))
                    .is_ok()
                {
                    return Some(outcome.to_owned());
                }
            }
        }
        record.goal_delivery.clone()
    }

    fn goal_result(&self, record: &AgentRecord, command: Option<&[String]>) -> Value {
        let mut result = json!({"name":record.name,"goal":record.goal,"delivery":self.goal_delivery(record),"source":"requested","native_status":"unverified"});
        if record.harness != "codex" {
            return result;
        }
        let Some(session) = record
            .goal_session_id
            .as_deref()
            .or(record.session_value.as_deref())
            .filter(|s| !s.is_empty())
        else {
            result["native_error"] = json!(
                "native session unknown; bind-session with the session id reported by this agent"
            );
            return result;
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
        result
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
            return Ok(self.goal_result(&record, goal_command));
        };
        if text.trim().is_empty() || text.contains(['\n', '\r']) {
            return Err(fail("goal must be a nonempty single line"));
        }
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        record.input_allowed()?;
        record.goal = Some(text.to_owned());
        record.goal_delivery = Some("pending".to_owned());
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_nanos();
        let identifier = format!("{timestamp:020}-{}", std::process::id());
        record.goal_message_id = Some(identifier.clone());
        if record.harness == "codex" {
            record
                .goal_messages
                .insert(identifier.clone(), text.to_owned());
        }
        self.save(&record)?;
        let prompt = if record.harness == "codex" {
            format!("/goal {text}")
        } else {
            format!("Your ongoing goal: {text}\nWork toward this goal and report completion or blockers.")
        };
        let queue = self.queue(agent_name)?;
        let client = WorkspaceClient {
            client: self.client,
            record: &record,
            goal_objective: Mutex::new(None),
            custom_submission: Mutex::new(None),
            queue: Some(&queue),
            check_prompt: true,
        };
        let outcome = agent::send_identified(
            &client,
            &record.target()?,
            &queue,
            &prompt,
            &identifier,
            options,
        );
        match outcome {
            Ok(delivered) => {
                record.goal_delivery = Some(delivered.outcome.as_str().to_owned());
                self.save(&record)?;
                Ok(self.goal_result(&record, goal_command))
            }
            Err(error) => {
                record.goal_delivery = Some(
                    error
                        .outcome()
                        .map_or("failed", |outcome| outcome.as_str())
                        .to_owned(),
                );
                self.save(&record)?;
                Err(error)
            }
        }
    }

    fn checked_or_launch_failed(&self, record: &AgentRecord, pane: &str) -> Result<()> {
        if record.adapter != "herdr-pane" {
            if record.lifecycle != "launch_failed" || self.client.pane_info(pane)?.agent.is_some() {
                self.checked(record)?;
            }
            return Ok(());
        }
        if matches!(record.lifecycle.as_str(), "starting" | "launch_failed")
            && record.custom_process_identity.is_some()
        {
            let info = self.client.pane_info(pane)?;
            let cwd_matches = info.cwd == record.cwd
                || fs::canonicalize(&info.cwd)
                    .ok()
                    .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.cwd).ok());
            if info.pane_id == pane
                && Some(&info.workspace_id) == record.workspace_id.as_ref()
                && cwd_matches
                && self
                    .client
                    .verify_custom_harness(
                        pane,
                        &record.harness,
                        record.custom_process_identity.as_ref(),
                    )
                    .is_ok()
            {
                return Ok(());
            }
        }
        if record.lifecycle == "launch_failed" && record.pane_reported_by_agentctl {
            let info = self.client.pane_info(pane)?;
            let cwd_matches = info.cwd == record.cwd
                || fs::canonicalize(&info.cwd)
                    .ok()
                    .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.cwd).ok());
            if info.pane_id == pane
                && Some(&info.workspace_id) == record.workspace_id.as_ref()
                && cwd_matches
                && info.agent.as_deref() == Some(&record.harness)
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
        let cwd_matches = info.cwd == record.cwd
            || fs::canonicalize(&info.cwd)
                .ok()
                .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.cwd).ok());
        if info.pane_id != pane_id || info.workspace_id != workspace_id || !cwd_matches {
            return Err(fail(format!(
                "refusing to {operation} {:?}: recorded pane, workspace, or cwd changed",
                record.name
            )));
        }
        if let Some(agent) = info.agent.as_deref() {
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
        if record.adapter != "herdr-foreign"
            || record.lifecycle != "running"
            || record.mode != "interactive"
            || record.backend != "herdr"
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
        if record.adapter != "herdr"
            || record.lifecycle != "running"
            || record.mode != "interactive"
            || record.backend != "herdr"
        {
            return Err(fail(
                "managed-dead retirement requires a running herdr record in interactive/herdr mode",
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
        let mut stopped_bytes = serde_json::to_vec_pretty(&final_record)
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
        self.client.close_pane(pane_id)?;
        atomic_replace_bytes(pinned, "agent.json", &stopped_bytes).map_err(|error| *error.error)?;
        self.publish_pinned_directory(pinned, &destination, &stopped_bytes)?;
        let tab_closed = self.client.panes().ok().map(|panes| {
            panes
                .iter()
                .all(|pane| Some(&pane.tab_id) != final_record.tab_id.as_ref())
        });
        Ok(json!({
            "name": record.name,
            "archive": destination,
            "pane_closed": true,
            "tab_closed": tab_closed,
            "managed_dead": true,
        }))
    }

    /// Close an owned pane, or only unregister a foreign runtime, then archive state.
    pub fn stop(&self, agent_name: &str) -> Result<Value> {
        self.stop_with_options(agent_name, StopOptions::default())
    }

    /// Stop with explicit generation assertions or the loud adoption-recovery gate.
    pub fn stop_with_options(&self, agent_name: &str, options: StopOptions) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
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
        if confirmed_record.token != record.token || json!(confirmed_record) != json!(record) {
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
        if record.adapter == "herdr-foreign" {
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
        if record.adapter == "herdr"
            && matches!(record.lifecycle.as_str(), "running" | "stopping")
            && record.pane_id.is_some()
        {
            let recorded: Vec<&Pane> = panes
                .iter()
                .filter(|pane| Some(&pane.pane_id) == record.pane_id.as_ref())
                .collect();
            if recorded.len() == 1 && self.client.pane_info(&recorded[0].pane_id)?.agent.is_none() {
                if record.lifecycle != "running" {
                    return Err(fail(
                        "managed-dead retirement requires a running herdr record",
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
                && (info.cwd == record.cwd
                    || fs::canonicalize(&info.cwd)
                        .ok()
                        .is_some_and(|cwd| Some(cwd) == fs::canonicalize(&record.cwd).ok()))
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
            if record.adapter == "herdr-pane"
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
                    custom_fails_after_identity: AtomicBool::new(false),
                    custom_at_idle_shell: AtomicBool::new(true),
                    foreign_shell_identity: Mutex::new(Fake::foreign_shell_identity()),
                    foreign_shell_path: Mutex::new(PathBuf::from("/bin/bash")),
                    replacement_shell_on_read: Mutex::new(None),
                    change_foreign_shell_after_save: AtomicBool::new(false),
                    change_foreign_shell_on_read: AtomicBool::new(false),
                    change_record_token_on_read: AtomicBool::new(false),
                    add_null_shell_identity_on_read: AtomicBool::new(false),
                    replace_record_directory_on_read: AtomicBool::new(false),
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
            let mut document = agent::read_private_json(&path).unwrap();
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
        custom_fails_after_identity: AtomicBool,
        custom_at_idle_shell: AtomicBool,
        foreign_shell_identity: Mutex<CustomProcessIdentity>,
        foreign_shell_path: Mutex<PathBuf>,
        replacement_shell_on_read: Mutex<Option<PaneShellProof>>,
        change_foreign_shell_after_save: AtomicBool,
        change_foreign_shell_on_read: AtomicBool,
        change_record_token_on_read: AtomicBool,
        add_null_shell_identity_on_read: AtomicBool,
        replace_record_directory_on_read: AtomicBool,
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
            Ok(self.panes.lock().unwrap().clone())
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
            } else if pane == "claude" {
                "claude"
            } else {
                "codex"
            };
            Ok(AgentPaneInfo {
                pane_id: pane.to_owned(),
                workspace_id: if pane == "external" {
                    "other-workspace"
                } else {
                    "workspace"
                }
                .to_owned(),
                cwd: if self.wrong_foreign_cwd.load(Ordering::Relaxed) {
                    self.root.join("other").display().to_string()
                } else {
                    self.root.display().to_string()
                },
                agent: self
                    .started
                    .load(Ordering::Relaxed)
                    .then(|| kind.to_owned()),
                status: "idle".to_owned(),
                session_agent: report_session.then(|| kind.to_owned()),
                session_value: report_session.then(|| {
                    if changed_after_save {
                        "replacement-thread"
                    } else if matches!(pane, "owned" | "claude" | "reported")
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
            Ok(())
        }
        fn wait_agent_status(&self, _: &str, _: &str, _: u64) -> AdapterResult<()> {
            Ok(())
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
            Ok("visible output".to_owned())
        }
    }
    impl ManagedApi for Fake {
        fn workspace_id_for_label(&self, _: &str) -> AdapterResult<Option<String>> {
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
            _: &[String],
            _: Duration,
            persist_identity: &mut dyn FnMut(CustomProcessIdentity) -> AdapterResult<()>,
        ) -> AdapterResult<()> {
            self.started.store(true, Ordering::Relaxed);
            self.custom_alive.store(true, Ordering::Relaxed);
            persist_identity(Self::custom_identity())?;
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
        fn verify_custom_harness(
            &self,
            _: &str,
            _: &str,
            identity: Option<&CustomProcessIdentity>,
        ) -> AdapterResult<()> {
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
            Ok("owned".to_owned())
        }
        fn report_agent_session(&self, _: &str, _: &str, _: &str, _: &str) -> AdapterResult<()> {
            Ok(())
        }
        fn send_keys(&self, _: &str, _: &str) -> AdapterResult<()> {
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
        assert!(!serde_json::to_string(&record).unwrap().contains(secret));
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
        assert_eq!(saved["adapter"], "herdr-foreign");
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
        assert!(saved["error"]
            .as_str()
            .unwrap()
            .contains("shell process identity changed"));
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
        let mut document = agent::read_private_json(&path).unwrap();
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
        let mut document = agent::read_private_json(&record_path).unwrap();
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
        let mut document = agent::read_private_json(&path).unwrap();
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
        document["adapter"] = json!("turn-runner");
        document["mode"] = json!("headless");
        document["backend"] = json!("tmux");
        document["runtime_home"] = json!(fixture.root.join("runtime"));
        document["session_agent"] = Value::Null;
        document["session_value"] = json!("thread");
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
        holder["adapter"] = json!("turn-runner");
        holder["mode"] = json!("headless");
        holder["backend"] = json!("tmux");
        holder["runtime_home"] = json!(fixture.root.join("runtime"));
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
            .goal_session_id
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
            .goal_session_id
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
            manager.load("foreign").unwrap().goal_session_id,
            manager.load("second").unwrap().goal_session_id,
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
    fn running_muse_record_does_not_gain_idle_shell_cleanup_fallback() {
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
        fixture.client.custom_alive.store(false, Ordering::Relaxed);
        let error = manager.stop("worker").unwrap_err();
        assert!(error.to_string().contains("foreground process"));
        assert!(fixture.client.closed.lock().unwrap().is_empty());
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
        wrong_adapter["adapter"] = json!("herdr");
        agent::atomic_json(&path, &wrong_adapter).unwrap();
        assert!(manager
            .load("foreign")
            .unwrap_err()
            .to_string()
            .contains("invalid agent record"));

        let mut legacy = original;
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
        document["future_padding"] = json!(vec!["x"; 150_000]);
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
        document["future_metadata"] = json!({"nested":true});
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
    fn foreign_runtime_metadata_is_visible_without_herdr_mutation() {
        let fixture = Fixture::new();
        fixture.start(None);
        let path = fixture.root.join("registry/worker/agent.json");
        let mut document = agent::read_private_json(&path).unwrap();
        document["adapter"] = json!("turn-runner");
        document["mode"] = json!("headless");
        document["runtime_home"] = json!("/tmp/worker-runtime");
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
        assert!(manager.stop("worker").is_err());
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }
}
