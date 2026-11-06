//! Recovery is a recorded launch followed by two pinned directory publications.
//! A durable journal reserves both generations until the old presentation is closed.

use super::*;

const SCHEMA: &str = "agentctl-revive/v1";

#[cfg(test)]
thread_local! {
    static FAILURE_POINT: Cell<Option<&'static str>> = const { Cell::new(None) };
    /// The Claude Code configuration directories a test pretends exist.
    pub(super) static CLAUDE_CONFIG_ROOTS: std::cell::RefCell<Option<Vec<PathBuf>>> =
        const { std::cell::RefCell::new(None) };
    /// The files a test pretends an agent's processes hold open.
    pub(super) static HELD_FILES: std::cell::RefCell<Option<Vec<PathBuf>>> =
        const { std::cell::RefCell::new(None) };
}

/// Where a record says why its native conversation is unknown (#916 muse-native-session-id).
pub(super) const NATIVE_SESSION_UNKNOWN: &str = "native_session_unknown";

/// The files the pinned process of `record`, and its descendants, hold open: none when the
/// record pins no process or the process is no longer the one pinned.
fn held_files(record: &AgentRecord) -> Vec<PathBuf> {
    // Tests never read the host's own processes: only what a test pretends.
    #[cfg(test)]
    return {
        let _ = record;
        HELD_FILES
            .with(|files| files.borrow().clone())
            .unwrap_or_default()
    };
    #[cfg(not(test))]
    {
        let Some(identity) = record
            .harness_anchor()
            .or(record.custom_process_identity.as_ref())
        else {
            return Vec::new();
        };
        let same = || {
            crate::client::live_custom_process(identity.pid)
                .is_ok_and(|observed| observed.identity == *identity)
        };
        let Ok(pid) = u32::try_from(identity.pid) else {
            return Vec::new();
        };
        if !same() {
            return Vec::new();
        }
        let files = crate::harness::transcript::Hints {
            session: None,
            cwd: None,
            home: None,
            pid: Some(pid),
            proc: Some(&crate::work_state::LinuxProc),
        }
        .open_files();
        // A pid reused while the descriptors were read would name another program's files.
        if same() {
            files
        } else {
            Vec::new()
        }
    }
}

/// Record the native conversation a just-started or just-adopted agent holds open, when the
/// record names none (#916 muse-native-session-id). Muse and Codex name their conversation by
/// the session files their runtime holds (Claude Code's is assigned at launch). One held
/// session is recorded as observed; several are never guessed between: the record keeps no
/// id and says why under [`NATIVE_SESSION_UNKNOWN`]. Returns what to tell the operator then.
pub(super) fn capture_held_session(record: &mut AgentRecord) -> Option<String> {
    if record.native_session.is_some() || !matches!(record.harness.as_str(), "muse" | "codex") {
        return None;
    }
    let adapter = crate::harness::adapter(&record.harness)?;
    // A runtime opens its session within a second or so of starting; a just-started one is
    // given a few seconds to (tests pretend, so they never wait).
    let tries = if cfg!(test) { 1 } else { 6 };
    let mut ids = Vec::new();
    for attempt in 0..tries {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        ids = adapter.held_sessions(&held_files(record));
        ids.retain(|id| valid_metadata_text(id));
        if !ids.is_empty() {
            break;
        }
    }
    match ids.as_slice() {
        [] => None,
        [id] => {
            let native = NativeSession::new(&record.harness, id, "observed");
            if native.valid_for(record) {
                record.native_session = Some(native);
                record.extra.remove(NATIVE_SESSION_UNKNOWN);
            }
            None
        }
        _ => {
            let reason = format!(
                "its processes hold {} {} sessions ({}), and agentctl does not guess which one is its own",
                ids.len(),
                record.harness,
                ids.join(", ")
            );
            record.extra.insert(
                NATIVE_SESSION_UNKNOWN.to_owned(),
                json!({"reason": reason, "candidates": ids}),
            );
            Some(format!(
                "native conversation of {} is unknown: {reason}; name it with `agentctl bind-session {} <id>`",
                record.name, record.name
            ))
        }
    }
}

/// Where Claude Code may keep transcripts: `$CLAUDE_CONFIG_DIR`, then `~/.claude`.
fn claude_config_roots() -> Vec<PathBuf> {
    // Tests never read the host's own configuration: only what a test pretends.
    #[cfg(test)]
    return CLAUDE_CONFIG_ROOTS
        .with(|roots| roots.borrow().clone())
        .unwrap_or_default();
    #[cfg(not(test))]
    crate::harness::claude::config_roots()
}

/// Whether a Claude Code transcript of `session` begun in `cwd` exists under any of `roots`:
/// `Some(true)` found, `Some(false)` missing from every root that has a `projects` directory,
/// `None` when no root has one (nowhere to look, so no verdict).
pub(super) fn claude_transcript_verdict(
    roots: &[PathBuf],
    cwd: &str,
    session: &str,
) -> Option<bool> {
    let encoded: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut looked = false;
    for root in roots {
        let projects = root.join("projects");
        if !projects.is_dir() {
            continue;
        }
        looked = true;
        if projects
            .join(&encoded)
            .join(format!("{session}.jsonl"))
            .is_file()
            || crate::inbox::watch::transcript_of_session(&projects, session).is_some()
        {
            return Some(true);
        }
    }
    looked.then_some(false)
}

/// Discover a Claude session for `record` from transcripts on disk (#741 restore-native-sessions):
/// the newest `<session>.jsonl` in the encoded project directory for the record's cwd whose
/// head mentions that cwd. Returns `None` when nothing confirms, so the caller keeps refusing.
pub(super) fn discover_claude_session(
    roots: &[PathBuf],
    record: &AgentRecord,
) -> Option<NativeSession> {
    if record.harness != "claude" {
        return None;
    }
    crate::harness::claude::project_transcripts(roots, Path::new(&record.cwd), &valid_metadata_text)
        .into_iter()
        .find(|transcript| {
            fs::read(&transcript.path).is_ok_and(|bytes| {
                String::from_utf8_lossy(&bytes[..bytes.len().min(65536)]).contains(&record.cwd)
            })
        })
        .map(|transcript| NativeSession::new("claude", &transcript.session, "observed"))
}

/// Discover a Codex session for `record` (#741): newest `rollout-*-<thread>.jsonl` under
/// `$CODEX_HOME/sessions` (default `~/.codex/sessions`) whose head mentions the record's cwd.
pub(super) fn discover_codex_session(record: &AgentRecord) -> Option<NativeSession> {
    if record.harness != "codex" {
        return None;
    }
    // Tests never scan the host's own Codex home.
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| crate::harness::home().map(|h| h.join(".codex")))?;
        discover_codex_in(&home.join("sessions"), record)
    }
}

#[cfg(not(test))]
fn discover_codex_in(sessions: &Path, record: &AgentRecord) -> Option<NativeSession> {
    let mut best: Option<(std::time::SystemTime, String)> = None;
    let mut stack = vec![sessions.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            let head = String::from_utf8_lossy(&bytes[..bytes.len().min(65536)]);
            if !head.contains(&record.cwd) {
                continue;
            }
            let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            // The session id comes from the first entry's session_meta; no id, no bind.
            let Some(value) = head
                .lines()
                .next()
                .and_then(|line| serde_json::from_str::<Value>(line).ok())
                .and_then(|v| {
                    v.pointer("/payload/id")
                        .or_else(|| v.pointer("/id"))
                        .and_then(|i| i.as_str().map(str::to_owned))
                })
            else {
                continue;
            };
            if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                best = Some((mtime, value));
            }
        }
    }
    best.map(|(_, value)| NativeSession::new("codex", &value, "observed"))
}

/// Newest `archive/<name>-<token>` directory for `name`, by modification time (#741).
pub(super) fn newest_archive(archive: &Path, name: &str) -> Option<PathBuf> {
    let prefix = format!("{name}-");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(archive).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !file_name.starts_with(&prefix) {
            continue;
        }
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
pub(super) fn fail_once(point: &'static str) {
    FAILURE_POINT.set(Some(point));
}

fn checkpoint(_point: &str) -> Result<()> {
    #[cfg(test)]
    if FAILURE_POINT.get() == Some(_point) {
        FAILURE_POINT.set(None);
        return Err(fail(format!("injected interruption after revive {_point}")));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Proof {
    kind: String,
    pane_id: String,
    tab_id: String,
    workspace_id: String,
    cwd: String,
    terminal_id: Option<String>,
    reported_agent: Option<String>,
    reported_session_agent: Option<String>,
    reported_session_value: Option<String>,
    shell: Option<CustomProcessIdentity>,
    shell_executable: Option<String>,
    /// Keys a newer agentctl added, kept when this record is written back (#809
    /// chat-bridge-json-forward-compat).
    #[serde(flatten)]
    unknown: crate::forward_compat::KeptKeys,
}

/// Authority to exclude exactly one dead presentation from this recovery's session census.
pub(super) struct CensusExclusion {
    old: AgentRecord,
    proof: Proof,
}

impl CensusExclusion {
    fn new(old: &AgentRecord, proof: &Proof) -> Self {
        Self {
            old: old.clone(),
            proof: proof.clone(),
        }
    }
}

struct RecoveryRouting<'m, 'a, A: ManagedApi + ?Sized, D: AgentApi + ?Sized> {
    manager: &'m ManagedAgents<'a, A>,
    delegate: &'m D,
    exclusion: &'m CensusExclusion,
    workspace_scope: Option<&'m str>,
}

impl<A: ManagedApi + ?Sized, D: AgentApi + ?Sized> RecoveryRouting<'_, '_, A, D> {
    fn verify_exclusion(&self) -> crate::error::Result<()> {
        let proof = self
            .manager
            .revive_proof(&self.exclusion.old)
            .map_err(|error| AdapterError::unavailable(error.to_string()))?;
        if proof != self.exclusion.proof {
            return Err(AdapterError::unavailable(
                "dead presentation proof changed during revive session census",
            ));
        }
        Ok(())
    }
}

impl<A: ManagedApi + ?Sized, D: AgentApi + ?Sized> AgentApi for RecoveryRouting<'_, '_, A, D> {
    fn panes(&self) -> crate::error::Result<Vec<Pane>> {
        self.verify_exclusion()?;
        let panes = self.delegate.panes()?;
        validate_census(&panes).map_err(|error| AdapterError::unavailable(error.to_string()))?;
        let old_present = panes
            .iter()
            .any(|pane| pane.pane_id == self.exclusion.proof.pane_id);
        if (self.exclusion.proof.kind == "missing" && old_present)
            || (self.exclusion.proof.kind == "idle_shell"
                && self
                    .workspace_scope
                    .is_none_or(|workspace| workspace == self.exclusion.proof.workspace_id)
                && !old_present)
        {
            return Err(AdapterError::unavailable(
                "dead presentation presence changed in revive session census",
            ));
        }
        if panes.iter().any(|pane| {
            pane.pane_id == self.exclusion.proof.pane_id
                && (pane.tab_id != self.exclusion.proof.tab_id
                    || pane.workspace_id != self.exclusion.proof.workspace_id)
        }) {
            return Err(AdapterError::unavailable(
                "dead presentation changed in revive session census",
            ));
        }
        self.verify_exclusion()?;
        Ok(panes
            .into_iter()
            .filter(|pane| pane.pane_id != self.exclusion.proof.pane_id)
            .collect())
    }

    fn pane_info(&self, pane: &str) -> crate::error::Result<AgentPaneInfo> {
        if pane == self.exclusion.proof.pane_id {
            return Err(AdapterError::unavailable(
                "recovery cannot target the retired presentation",
            ));
        }
        self.verify_exclusion()?;
        let info = self.delegate.pane_info(pane)?;
        self.verify_exclusion()?;
        Ok(info)
    }

    fn workspace_label(&self, workspace: &str) -> crate::error::Result<String> {
        self.verify_exclusion()?;
        let label = self.delegate.workspace_label(workspace)?;
        self.verify_exclusion()?;
        Ok(label)
    }

    fn run(&self, _pane: &str, _text: &str) -> crate::error::Result<()> {
        Err(AdapterError::unavailable(
            "revive session census cannot submit input",
        ))
    }

    fn wait_agent_status(
        &self,
        _pane: &str,
        _status: &str,
        _timeout: u64,
    ) -> crate::error::Result<()> {
        Err(AdapterError::unavailable(
            "revive session census cannot wait for input effects",
        ))
    }

    fn read(
        &self,
        _pane: &str,
        _source: &str,
        _lines: Option<usize>,
    ) -> crate::error::Result<String> {
        Err(AdapterError::unavailable(
            "revive session census cannot read terminal output",
        ))
    }
}

impl Proof {
    fn valid(&self) -> bool {
        [
            self.pane_id.as_str(),
            self.tab_id.as_str(),
            self.workspace_id.as_str(),
            self.cwd.as_str(),
        ]
        .into_iter()
        .all(valid_metadata_text)
            && Path::new(&self.cwd).is_absolute()
            && match self.kind.as_str() {
                "missing" => {
                    self.terminal_id.is_none()
                        && self.reported_agent.is_none()
                        && self.reported_session_agent.is_none()
                        && self.reported_session_value.is_none()
                        && self.shell.is_none()
                        && self.shell_executable.is_none()
                }
                "idle_shell" => {
                    self.shell
                        .as_ref()
                        .is_some_and(CustomProcessIdentity::valid)
                        && self.shell_executable.as_deref().is_some_and(|path| {
                            valid_metadata_text(path) && Path::new(path).is_absolute()
                        })
                        && self.terminal_id.as_deref().is_none_or(valid_metadata_text)
                        && self
                            .reported_agent
                            .as_deref()
                            .is_none_or(valid_metadata_text)
                        && self
                            .reported_session_agent
                            .as_deref()
                            .is_none_or(valid_metadata_text)
                        && self
                            .reported_session_value
                            .as_deref()
                            .is_none_or(valid_metadata_text)
                }
                _ => false,
            }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Journal {
    #[serde(skip)]
    expected_bytes: Vec<u8>,
    schema: String,
    name: String,
    old_token: String,
    new_token: String,
    phase: String,
    started_at: f64,
    old_directory_device: u64,
    old_directory_inode: u64,
    new_directory_device: u64,
    new_directory_inode: u64,
    old_record_sha256: String,
    stopped_record_sha256: String,
    ready_record_sha256: Option<String>,
    proof: Proof,
    /// Keys a newer agentctl added, kept when this record is written back (#809
    /// chat-bridge-json-forward-compat).
    #[serde(flatten)]
    unknown: crate::forward_compat::KeptKeys,
}

pub(super) fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub(super) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn artifact_name_allowed(pinned: &PinnedAgentDirectory, file_name: &str) -> bool {
    if pinned.path.file_name().and_then(|value| value.to_str()) == Some(".revives") {
        return file_name.strip_suffix(".json").is_some_and(token);
    }
    file_name == "stopped.json"
        && pinned
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(token)
        && pinned
            .path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            == Some(".revives")
}

fn document_bytes_with_limit(value: &impl Serialize, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| fail("cannot serialize revive recovery metadata"))?;
    bytes.push(b'\n');
    if bytes.len() > limit {
        return Err(fail(
            "revive recovery metadata exceeds the record size limit",
        ));
    }
    Ok(bytes)
}

fn document_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    document_bytes_with_limit(value, MAX_AGENT_RECORD_BYTES)
}

pub(super) fn record_document_bytes(record: &AgentRecord) -> Result<Vec<u8>> {
    document_bytes(record)
}

pub(super) fn pin_directory(path: &Path, name: &str) -> Result<PinnedAgentDirectory> {
    let pinned = ManagedAgents::<HerdrClient>::pinned_parent_directory(path, "revive directory")?;
    Ok(PinnedAgentDirectory {
        name: name.to_owned(),
        path: pinned.path,
        file: pinned.file,
        device: pinned.device,
        inode: pinned.inode,
    })
}

fn artifact_bytes(pinned: &PinnedAgentDirectory, file_name: &str) -> Result<Vec<u8>> {
    ManagedAgents::<HerdrClient>::verify_pinned_agent_directory(pinned)?;
    let mut file =
        ManagedAgents::<HerdrClient>::open_pinned_file(pinned, file_name, libc::O_RDONLY)?;
    let before = file.metadata().map_err(|error| fail(error.to_string()))?;
    let uid = unsafe { libc::getuid() };
    if !before.is_file()
        || before.uid() != uid
        || before.mode() & 0o077 != 0
        || before.nlink() != 1
        || before.len() > MAX_AGENT_RECORD_BYTES as u64
    {
        return Err(fail("unsafe revive recovery artifact"));
    }
    let mut content = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_AGENT_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut content)
        .map_err(|error| fail(error.to_string()))?;
    let after = file.metadata().map_err(|error| fail(error.to_string()))?;
    if content.len() > MAX_AGENT_RECORD_BYTES
        || content.len() as u64 != before.len()
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.mode() != after.mode()
        || before.uid() != after.uid()
        || before.nlink() != after.nlink()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(fail("revive recovery artifact changed while being read"));
    }
    ManagedAgents::<HerdrClient>::verify_pinned_agent_directory(pinned)?;
    Ok(content)
}

pub(super) fn read_record_at(
    path: &Path,
    name: &str,
) -> Result<(PinnedAgentDirectory, AgentRecord, Vec<u8>)> {
    let pinned = pin_directory(path, name)?;
    let bytes = artifact_bytes(&pinned, "agent.json")?;
    let record = match nested::decode_bytes(&bytes, &path.join("agent.json")) {
        Some(record) => record?,
        None => serde_json::from_slice(&bytes).map_err(|_| fail("invalid staged revive record"))?,
    };
    record.validate_loaded(&path.join("agent.json"), name)?;
    Ok((pinned, record, bytes))
}

fn optional_record_at(
    path: &Path,
    name: &str,
) -> Result<Option<(PinnedAgentDirectory, AgentRecord, Vec<u8>)>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fail(error.to_string())),
        Ok(_) => read_record_at(path, name).map(Some),
    }
}

/// Where abandoned revive attempts are kept as evidence, one directory each (#899
/// revive-abandon-dead-candidate). Its leading dot keeps it out of the journal scan.
const ABANDONED: &str = ".abandoned";

/// What an interrupted revive's candidate shows when checked independently (#899).
#[derive(Debug)]
enum CandidateCheck {
    /// No harness can be running for it; the evidence says why.
    Dead(String),
    /// A harness process runs for it; the evidence names it.
    Live(String),
    /// Neither can be proved.
    Unproven(String),
}

fn operation_directory(registry: &Path, journal: &Journal) -> PathBuf {
    registry.join(".revives").join(&journal.old_token)
}

fn parse_journal(path: &Path) -> Result<Journal> {
    let invalid = || fail(format!("invalid revive journal: {}", path.display()));
    let root = pin_directory(path.parent().ok_or_else(invalid)?, "revive")?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(invalid)?;
    let content = artifact_bytes(&root, file_name)?;
    let mut journal: Journal = serde_json::from_slice(&content).map_err(|_| invalid())?;
    journal.expected_bytes = content;
    if journal.schema != SCHEMA
        || !name_pattern(&journal.name)
        || !token(&journal.old_token)
        || !token(&journal.new_token)
        || journal.old_token == journal.new_token
        || path.file_name().and_then(|value| value.to_str())
            != Some(&format!("{}.json", journal.old_token))
        || !matches!(journal.phase.as_str(), "launching" | "ready" | "published")
        || !journal.started_at.is_finite()
        || journal.old_directory_device == 0
        || journal.old_directory_inode == 0
        || journal.new_directory_device == 0
        || journal.new_directory_inode == 0
        || !valid_digest(&journal.old_record_sha256)
        || !valid_digest(&journal.stopped_record_sha256)
        || journal
            .ready_record_sha256
            .as_deref()
            .is_some_and(|value| !valid_digest(value))
        || (journal.phase != "launching" && journal.ready_record_sha256.is_none())
        || !journal.proof.valid()
    {
        return Err(invalid());
    }
    Ok(journal)
}

fn journals(registry: &Path) -> Result<Vec<Journal>> {
    let directory = registry.join(".revives");
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(fail(error.to_string())),
        Ok(_) => {}
    }
    agent::validate_private_directory(&directory, "revive journal directory", false)?;
    let mut paths = fs::read_dir(&directory)
        .map_err(|error| fail(error.to_string()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| fail(error.to_string()))?;
    paths.sort();
    let mut names = BTreeSet::new();
    let mut result = Vec::new();
    for path in paths {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if file_name.starts_with('.') {
            continue;
        }
        if token(file_name) && path.is_dir() {
            agent::validate_private_directory(&path, "revive operation directory", false)?;
            continue;
        }
        if !file_name.ends_with(".json") {
            return Err(fail("unexpected entry in revive journal directory"));
        }
        let journal = parse_journal(&path)?;
        if !names.insert(journal.name.clone()) {
            return Err(fail("multiple revive journals reserve the same name"));
        }
        result.push(journal);
    }
    Ok(result)
}

fn stopped_artifact(registry: &Path, journal: &Journal) -> Result<(AgentRecord, Vec<u8>)> {
    let operation = pin_directory(&operation_directory(registry, journal), &journal.name)?;
    let bytes = artifact_bytes(&operation, "stopped.json")?;
    if digest(&bytes) != journal.stopped_record_sha256 {
        return Err(fail("prepared stopped revive record changed"));
    }
    let record = match nested::decode_bytes(&bytes, &operation.path.join("stopped.json")) {
        Some(record) => record?,
        None => serde_json::from_slice(&bytes)
            .map_err(|_| fail("invalid prepared stopped revive record"))?,
    };
    record.validate_loaded(&operation.path.join("stopped.json"), &journal.name)?;
    if record.token != journal.old_token
        || record.lifecycle != "stopped"
        || record.pane_id.as_deref() != Some(&journal.proof.pane_id)
        || record.tab_id.as_deref() != Some(&journal.proof.tab_id)
        || record.workspace_id.as_deref() != Some(&journal.proof.workspace_id)
        || !same_directory(&record.cwd, &journal.proof.cwd)
    {
        return Err(fail(
            "prepared stopped revive record does not match its journal",
        ));
    }
    Ok((record, bytes))
}

/// Whether a directory read now is the directory a revive journal pinned.
///
/// Only the inode is compared: `st_dev` is not stable for the same filesystem
/// across a reboot or remount (btrfs assigns anonymous device numbers per
/// mount), so a journal written before a reboot otherwise never matches the
/// directory it pinned and every revive in the registry is refused.
///
/// Dropping the device is safe against a different directory reusing the
/// inode because the inode only locates the directory; it does not identify
/// the generation. Every comparison through this helper also requires the
/// pinned generation token, and, where the journal recorded one, the exact
/// record bytes or their SHA-256 (the remaining sites check the digest in
/// the same refusal). A reused inode on another filesystem would therefore
/// also have to hold a record carrying the pinned token and digest, which
/// is the pinned generation itself, not a stand-in for it. Checks within a
/// single run keep comparing device and inode together, where the device
/// cannot have changed.
fn same_journal_directory(inode: u64, journal_inode: u64) -> bool {
    inode == journal_inode
}

/// Pending generations reserve their old and staged anchors even after a failed launch.
pub(super) fn pending_claim_records(registry: &Path) -> Result<Vec<AgentRecord>> {
    let mut result = Vec::new();
    for journal in journals(registry)? {
        stopped_artifact(registry, &journal)?;
        let archive = registry
            .join("archive")
            .join(format!("{}-{}", journal.name, journal.old_token));
        let archived = match fs::symlink_metadata(&archive) {
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(fail(error.to_string())),
        };
        let old_path = if archived {
            archive
        } else {
            agent_queue::registry::directory(registry, &journal.name)
                .map_err(|error| fail(error.to_string()))?
        };
        let (old_directory, old, old_bytes) = read_record_at(&old_path, &journal.name)?;
        let old_digest = digest(&old_bytes);
        if old.token != journal.old_token
            || !same_journal_directory(old_directory.inode, journal.old_directory_inode)
            || ![
                journal.old_record_sha256.as_str(),
                journal.stopped_record_sha256.as_str(),
            ]
            .contains(&old_digest.as_str())
            || (archived && old_digest != journal.stopped_record_sha256)
        {
            return Err(fail(
                "old revive generation changed while reserving identities",
            ));
        }
        result.push(old);
        let candidate = operation_directory(registry, &journal).join("new");
        let candidate = match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                agent_queue::registry::directory(registry, &journal.name)
                    .map_err(|error| fail(error.to_string()))?
            }
            Err(error) => return Err(fail(error.to_string())),
            Ok(_) => candidate,
        };
        let (pinned, record, bytes) = read_record_at(&candidate, &journal.name)?;
        if !same_journal_directory(pinned.inode, journal.new_directory_inode)
            || record.token != journal.new_token
            || journal
                .ready_record_sha256
                .as_ref()
                .is_some_and(|expected| expected != &digest(&bytes))
        {
            return Err(fail("pending revive generation changed"));
        }
        result.push(record);
    }
    Ok(result)
}

struct LaunchPlan {
    options: StartOptions,
    arguments: Vec<String>,
    native: NativeSession,
    project_workspace: Option<String>,
    slot_command: Option<String>,
    relay_command: Option<String>,
    relay_executable: Option<String>,
}

fn conversation(record: &AgentRecord) -> Result<NativeSession> {
    if !record.session_asserted()
        && (record.session_agent.is_some() != record.session_value.is_some()
            || record
                .session_agent
                .as_deref()
                .is_some_and(|agent| agent != record.harness))
    {
        return Err(fail(
            "recorded native conversation provider is incomplete or conflicts with the harness",
        ));
    }
    let native = record
        .native_session
        .clone()
        .or_else(|| {
            record
                .resume
                .as_deref()
                .map(|value| NativeSession::new(&record.harness, value, "asserted"))
        })
        .or_else(|| {
            record.session_value.as_deref().map(|value| {
                NativeSession::new(
                    record.session_agent.as_deref().unwrap_or(&record.harness),
                    value,
                    "observed",
                )
            })
        })
        .or_else(|| discover_claude_session(&claude_config_roots(), record))
        .or_else(|| discover_codex_session(record))
        .ok_or_else(|| {
            match record
                .extra
                .get(NATIVE_SESSION_UNKNOWN)
                .and_then(|note| note["reason"].as_str())
            {
                Some(reason) => fail(format!(
                    "record has no native conversation ID: {reason}; name it with `agentctl bind-session {} <id>`",
                    record.name
                )),
                None => fail("record has no native conversation ID"),
            }
        })?;
    if !native.valid_for(record) {
        return Err(fail("record has contradictory native conversation IDs"));
    }
    Ok(native)
}

/// Remove only the structured selector whose provenance is recorded.
fn policy_arguments(record: &AgentRecord, native: &NativeSession) -> Result<Vec<String>> {
    let mut arguments = record.arguments.clone();
    let selector = match record.harness.as_str() {
        "claude" if record.resume.is_some() => {
            Some(vec!["--resume".to_owned(), native.value.clone()])
        }
        "claude" if arguments.first().map(String::as_str) == Some("--session-id") => {
            Some(vec!["--session-id".to_owned(), native.value.clone()])
        }
        "codex" if record.resume.is_some() => Some(vec!["resume".to_owned(), native.value.clone()]),
        _ => None,
    };
    if let Some(selector) = selector {
        if !arguments.starts_with(&selector) {
            return Err(fail(
                "recorded conversation selector does not match launch arguments",
            ));
        }
        arguments.drain(..selector.len());
    }
    if record.harness == "muse" && record.resume.is_some() {
        let selector = ["resume".to_owned(), native.value.clone()];
        if !arguments.ends_with(&selector) {
            return Err(fail(
                "recorded Muse resume selector does not match launch arguments",
            ));
        }
        arguments.truncate(arguments.len() - selector.len());
    }
    if record.harness == "claude"
        && arguments
            .iter()
            .any(|argument| claude_conversation_selector(argument))
    {
        return Err(fail(
            "recorded Claude arguments contain an unstructured conversation selector",
        ));
    }
    Ok(arguments)
}

/// The record key that asks the next revive to start a fresh conversation instead of resuming
/// the recorded one (#686 agent-turnover): `{"schema", "session", "reason"}`. It lives in the
/// dead record, so a revive interrupted and finished later still starts the same fresh
/// conversation; the revived record does not carry it.
pub(super) const FRESH_REVIVE: &str = "fresh_revive";
const FRESH_REVIVE_SCHEMA: &str = "agentctl-fresh-revive/v1";

/// The fresh conversation (session, reason) a record asks its next revive to start.
pub(super) fn fresh_revive(record: &AgentRecord) -> Option<(String, String)> {
    let marker = record
        .extra
        .get(FRESH_REVIVE)
        .filter(|marker| marker["schema"] == FRESH_REVIVE_SCHEMA)?;
    let session = marker["session"]
        .as_str()
        .filter(|session| valid_metadata_text(session))?;
    Some((
        session.to_owned(),
        marker["reason"].as_str().unwrap_or_default().to_owned(),
    ))
}

/// The conversation a revive starts: a requested fresh one, else the recorded one.
fn revive_conversation(record: &AgentRecord) -> Result<NativeSession> {
    match fresh_revive(record) {
        Some((session, _)) => Ok(NativeSession::new(&record.harness, &session, "asserted")),
        None => recorded_conversation(record),
    }
}

/// The conversation a revive resumes: the launch spec's, else the record's own.
fn recorded_conversation(record: &AgentRecord) -> Result<NativeSession> {
    match record
        .launch_spec
        .as_ref()
        .and_then(|spec| spec["native_session"]["value"].as_str())
    {
        Some(value) => Ok(NativeSession::new(&record.harness, value, "asserted")),
        None => conversation(record),
    }
}

/// agentctl launches the replacement itself, so a revived adopted agent is owned.
fn revived_adapter(record: &AgentRecord) -> &str {
    match record.adapter.as_str() {
        "herdr-foreign" => "herdr",
        "tmux-foreign" => "tmux",
        _ => &record.adapter,
    }
}

/// The recorded spec's argument vector, if the record has a spec.
fn spec_arguments(record: &AgentRecord) -> Option<Vec<String>> {
    record.launch_spec.as_ref().map(|spec| {
        spec["argv"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()
    })
}

/// The exact harness arguments a revive of `record` launches.
fn expected_arguments(record: &AgentRecord) -> Result<Vec<String>> {
    if let Some((session, _)) = fresh_revive(record) {
        if let Some(argv) = spec_arguments(record) {
            return launch_spec::fresh_arguments(&record.harness, &argv, &session);
        }
        // A fresh Muse with no recorded conversation has no selector to strip (#741).
        let recorded = match recorded_conversation(record) {
            Ok(native) => native,
            Err(_error) if record.harness == "muse" => {
                NativeSession::new("muse", &session, "asserted")
            }
            Err(error) => return Err(error),
        };
        let policy = policy_arguments(record, &recorded)?;
        return fresh_selector(&record.harness, policy, &session);
    }
    let native = recorded_conversation(record)?;
    if let Some(argv) = spec_arguments(record) {
        return launch_spec::replay_arguments(&record.harness, &argv, &native.value);
    }
    let policy = policy_arguments(record, &native)?;
    Ok(if record.harness == "muse" {
        policy
            .into_iter()
            .chain(["resume".to_owned(), native.value.clone()])
            .collect()
    } else {
        [
            if record.harness == "claude" {
                "--resume"
            } else {
                "resume"
            }
            .to_owned(),
            native.value.clone(),
        ]
        .into_iter()
        .chain(policy)
        .collect()
    })
}

/// `arguments` with the selector that starts the fresh conversation `session` in front.
fn fresh_selector(harness: &str, arguments: Vec<String>, session: &str) -> Result<Vec<String>> {
    crate::harness::adapter(harness)
        .and_then(|adapter| adapter.fresh_arguments(arguments, session))
        .ok_or_else(|| {
            fail(format!(
                "{harness} cannot be started on a conversation ID agentctl chooses, so it cannot be revived fresh"
            ))
        })
}

/// The project's configured wrkslots, unless `AGENTCTL_WRKSLOTS_BIN` already names one:
/// the `wrkslots` key of profiles.json, else the team folder's `bin/wrkslots` (#742).
/// PATH is never consulted: a non-login shell's PATH silently hung restore for 300 s.
pub(super) fn configured_wrkslots(registry: &Path) -> Result<Option<PathBuf>> {
    if std::env::var_os(WRKSLOTS_BIN_ENV).is_some_and(|value| !value.is_empty()) {
        return Ok(None);
    }
    if let Some(configured) = crate::profiles::wrkslots_for_registry(registry)? {
        return Ok(Some(PathBuf::from(configured)));
    }
    let team = registry
        .parent()
        .map(|root| root.join("bin").join("wrkslots"));
    Ok(team.filter(|candidate| fs::metadata(candidate).is_ok_and(|metadata| metadata.is_file())))
}

fn validate_census(panes: &[Pane]) -> Result<()> {
    let mut ids = BTreeSet::new();
    if panes.iter().any(|pane| {
        !valid_metadata_text(&pane.pane_id)
            || !valid_metadata_text(&pane.tab_id)
            || !valid_metadata_text(&pane.workspace_id)
            || !ids.insert(pane.pane_id.as_str())
    }) {
        return Err(fail("pane census is incomplete or ambiguous"));
    }
    Ok(())
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    pub(super) fn resolve_recovery_target<D: AgentApi + ?Sized>(
        &self,
        client: &D,
        target: &Target,
        exclusion: Option<&CensusExclusion>,
        workspace_scope: Option<&str>,
    ) -> Result<AgentPaneInfo> {
        match exclusion.filter(|excluded| excluded.proof.reported_session_value.is_some()) {
            Some(exclusion) => agent::resolve_target(
                &RecoveryRouting {
                    manager: self,
                    delegate: client,
                    exclusion,
                    workspace_scope,
                },
                target,
            ),
            None => agent::resolve_target(client, target),
        }
    }

    pub(super) fn refuse_pending_revive(&self, names: &[&str]) -> Result<()> {
        self.refuse_pending_revive_with_stop_advice(names, None)
    }

    pub(super) fn refuse_pending_revive_with_stop_advice(
        &self,
        names: &[&str],
        recovery: Option<&mut RecoveryAction>,
    ) -> Result<()> {
        if let Some(journal) = journals(&self.registry)?
            .into_iter()
            .find(|journal| names.contains(&journal.name.as_str()))
        {
            let for_stop = recovery.is_some();
            if let Some(recovery) = recovery {
                *recovery = RecoveryAction::Revive {
                    name: journal.name.clone(),
                    token: journal.old_token.clone(),
                };
            }
            return Err(fail(if for_stop {
                format!("revive of '{}' is incomplete", journal.name)
            } else {
                format!(
                    "revive of '{}' is incomplete; rerun `agentctl revive {} --expected-token {}`",
                    journal.name, journal.name, journal.old_token
                )
            }));
        }
        Ok(())
    }

    fn revive_launch_plan(
        &self,
        record: &AgentRecord,
        startup_timeout: Duration,
        reuse_pane: Option<String>,
    ) -> Result<LaunchPlan> {
        if record.lifecycle != "running" {
            return Err(fail("record lifecycle is not running"));
        }
        if record.mode != "interactive"
            || record.backend != self.client.record_backend()
            || !matches!(
                record.adapter.as_str(),
                "herdr" | "herdr-pane" | "herdr-relay" | "herdr-foreign" | "tmux" | "tmux-foreign"
            )
            || !matches!(record.harness.as_str(), "claude" | "codex" | "muse")
        {
            return Err(fail(
                "revive requires a local interactive Claude, Codex, or Muse agent",
            ));
        }
        if record.is_foreign() && record.launch_spec.is_none() {
            return Err(fail(format!(
                "adopted record has no launch spec; while its harness runs, capture one with `agentctl anchor {} --replace`",
                record.name
            )));
        }
        if self.pending_move_destination(record)?.is_some() {
            return Err(fail("record has an incomplete move; finish the move first"));
        }
        if self
            .rename_journals()?
            .iter()
            .any(|journal| journal.names(&[&record.name]))
        {
            return Err(fail(
                "record has an incomplete rename; finish the rename first",
            ));
        }
        let cwd = fs::canonicalize(&record.cwd)
            .map_err(|_| fail("recorded starting directory is unavailable"))?;
        if !cwd.is_dir() || cwd != Path::new(&record.cwd) {
            return Err(fail("recorded starting directory changed"));
        }
        if let Some(spec) = record.launch_spec.as_ref() {
            return self.spec_launch_plan(record, spec, cwd, startup_timeout, reuse_pane);
        }
        // Muse with no recorded conversation starts fresh with a handoff brief (#741).
        let auto_fresh_muse = record.harness == "muse"
            && fresh_revive(record).is_none()
            && conversation(record).is_err();
        let recorded = match conversation(record) {
            Ok(native) => native,
            Err(_error) if auto_fresh_muse => {
                NativeSession::new("muse", &new_conversation_id()?, "asserted")
            }
            Err(error) => return Err(error),
        };
        let policy = policy_arguments(record, &recorded)?;
        let fresh = fresh_revive(record).or_else(|| {
            auto_fresh_muse.then(|| {
                (
                    recorded.value.clone(),
                    "Muse conversation was not recorded; restarted fresh".to_owned(),
                )
            })
        });
        let native = match &fresh {
            Some((session, _)) => NativeSession::new(&record.harness, session, "asserted"),
            None => recorded,
        };
        let mut options = StartOptions {
            harness: record.harness.clone(),
            profile: record.profile.clone(),
            model: record.model.clone(),
            resume: if auto_fresh_muse {
                None
            } else {
                Some(native.value.clone())
            },
            startup_timeout,
            ..StartOptions::default()
        };
        options.reuse_pane = reuse_pane;
        if auto_fresh_muse {
            options.brief = Some(format!(
                "You are {name}, restarted fresh after an outage because Muse cannot resume \
                 the previous conversation. Working directory: {cwd}. Check `agentctl status \
                 {name}` and your queued messages (`agentctl prompts {name}`), then continue \
                 your previous task from the repository state.",
                name = record.name,
                cwd = record.cwd
            ));
        }
        let base = harness_arguments(&record.harness, record.model.as_deref(), None, &[])?;
        if !policy.starts_with(&base) {
            return Err(fail("recorded harness policy arguments are inconsistent"));
        }
        let effort = crate::profiles::reasoning_arguments(
            &record.harness,
            record.reasoning_effort.as_deref(),
        )?;
        let mut extras = policy[base.len()..].to_vec();
        if !extras.starts_with(&effort) {
            return Err(fail(
                "recorded reasoning effort does not match launch arguments",
            ));
        }
        extras.drain(..effort.len());
        if let Some(profile_name) = record.profile.as_deref() {
            let root = crate::profiles::configuration_root(&cwd, &self.registry, false)?;
            let (_, profiles) = crate::profiles::load_profiles(&root, false)?;
            let profile = profiles
                .get(profile_name)
                .ok_or_else(|| fail("recorded launch profile is unavailable"))?;
            if profile.harness != record.harness
                || profile.mode != record.mode
                || profile.model != record.model
                || profile.reasoning_effort != record.reasoning_effort
                || profile.argv != extras
                || environment_names(&profile.environment) != record.environment_names
                || profile.cloud.is_some()
            {
                return Err(fail("recorded launch profile changed (harness, mode, model, effort, argv, or environment names)"));
            }
            options.environment = profile.environment.clone();
            options.harness_args = profile.argv.clone();
        } else {
            if !record.environment_names.is_empty() {
                return Err(fail(
                    "recorded environment cannot be replayed safely without its launch profile",
                ));
            }
            options.harness_args = extras;
        }
        crate::profiles::validate_structured_harness_argument_conflicts(
            "revive policy",
            &record.harness,
            &options.harness_args,
            record.model.is_some(),
            record.reasoning_effort.is_some(),
            true,
        )?;
        let mut structured = effort;
        structured.extend(options.harness_args.iter().cloned());
        let arguments = match &fresh {
            Some((session, _)) => fresh_selector(
                &record.harness,
                harness_arguments(&record.harness, record.model.as_deref(), None, &structured)?,
                session,
            )?,
            None => harness_arguments(
                &record.harness,
                record.model.as_deref(),
                Some(&native.value),
                &structured,
            )?,
        };
        self.finish_launch_plan(record, options, arguments, native, cwd)
    }

    /// Replay what ran: the spec's arguments with one explicit resume, and the environment
    /// values its names select from the private profile. A profile edited after launch does
    /// not change the replay.
    fn spec_launch_plan(
        &self,
        record: &AgentRecord,
        spec: &Value,
        cwd: PathBuf,
        startup_timeout: Duration,
        reuse_pane: Option<String>,
    ) -> Result<LaunchPlan> {
        let names = spec["environment_names"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        if spec["cwd"] != record.cwd
            || spec["profile"].as_str() != record.profile.as_deref()
            || spec["slot"].as_str() != record.slot.as_deref()
            || spec["slot_project"].as_str() != record.slot_project.as_deref()
            || spec["slot_isolation"].as_str() != record.slot_isolation.as_deref()
            || names
                != record
                    .environment_names
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
        {
            return Err(fail("recorded launch spec contradicts its record (cwd, profile, slot, or environment names)"));
        }
        let executable = spec["executable"].as_str();
        if !launch_spec::program_matches(&record.harness, executable) {
            return Err(fail(format!(
                "recorded program {:?} is not the {} harness, so it cannot be replayed",
                executable.unwrap_or_default(),
                record.harness
            )));
        }
        let native = revive_conversation(record)?;
        let mut options = StartOptions {
            harness: record.harness.clone(),
            profile: record.profile.clone(),
            model: record.model.clone(),
            resume: Some(native.value.clone()),
            startup_timeout,
            ..StartOptions::default()
        };
        options.reuse_pane = reuse_pane;
        if !names.is_empty() {
            let Some(profile_name) = record.profile.as_deref() else {
                return Err(fail(
                    "recorded environment cannot be replayed safely without its launch profile",
                ));
            };
            let root = crate::profiles::configuration_root(&cwd, &self.registry, false)?;
            let (_, profiles) = crate::profiles::load_profiles(&root, false)?;
            let profile = profiles
                .get(profile_name)
                .ok_or_else(|| fail("recorded launch profile is unavailable"))?;
            if environment_names(&profile.environment) != names {
                return Err(fail(
                    "recorded environment names cannot be reconstructed from the private launch profile",
                ));
            }
            options.environment = profile.environment.clone();
        }
        let argv = spec_arguments(record).unwrap_or_default();
        let arguments = if fresh_revive(record).is_some() {
            launch_spec::fresh_arguments(&record.harness, &argv, &native.value)?
        } else {
            launch_spec::replay_arguments(&record.harness, &argv, &native.value)?
        };
        self.finish_launch_plan(record, options, arguments, native, cwd)
    }

    /// The slot boxing, workspace policy, and identity checks every revive plan shares.
    fn finish_launch_plan(
        &self,
        record: &AgentRecord,
        mut options: StartOptions,
        arguments: Vec<String>,
        native: NativeSession,
        cwd: PathBuf,
    ) -> Result<LaunchPlan> {
        let mut slot_command = None;
        let mut relay_command = None;
        let mut relay_executable = None;
        #[cfg(test)]
        let slot_executable = self.revive_wrkslots_executable.clone();
        #[cfg(not(test))]
        let slot_executable = configured_wrkslots(&self.registry)?;
        let slot = if let Some(slot) = record.slot.as_ref() {
            Some(SlotLaunch {
                slot: slot.clone(),
                isolation: Some(
                    record
                        .slot_isolation
                        .clone()
                        .ok_or_else(|| fail("recorded slot isolation is unknown"))?,
                ),
                project: Some(PathBuf::from(
                    record
                        .slot_project
                        .as_ref()
                        .ok_or_else(|| fail("recorded slot project is unknown"))?,
                )),
                executable: slot_executable.clone(),
                ..SlotLaunch::default()
            })
        } else if let Some(boxed) = record.extra.get("project_box") {
            let object = boxed
                .as_object()
                .ok_or_else(|| fail("recorded project box policy is invalid"))?;
            let text = |key| {
                object
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| valid_metadata_text(value))
            };
            let scope = text("writable")
                .filter(|scope| BOX_SCOPES.contains(scope))
                .ok_or_else(|| fail("recorded effective project box writable scope is unknown"))?;
            Some(SlotLaunch {
                slot: text("name")
                    .ok_or_else(|| fail("recorded project box name is unknown"))?
                    .to_owned(),
                isolation: Some(
                    text("isolation")
                        .ok_or_else(|| fail("recorded project box isolation is unknown"))?
                        .to_owned(),
                ),
                project: Some(PathBuf::from(
                    text("project")
                        .ok_or_else(|| fail("recorded project box project is unknown"))?,
                )),
                project_box: true,
                box_writable: Some(scope.to_owned()),
                box_cwd: Some(cwd.clone()),
                executable: slot_executable,
            })
        } else {
            None
        };
        if let Some(slot) = slot.as_ref() {
            let project = slot.project.as_ref().expect("recorded project");
            let canonical = fs::canonicalize(project)
                .map_err(|_| fail("recorded slot project is unavailable"))?;
            if canonical != *project {
                return Err(fail("recorded slot project changed"));
            }
            let (line, slot_path, isolation) = slot_shell_command(slot, project, &[])?;
            if fs::canonicalize(slot_path).ok().as_ref() != Some(&cwd)
                || Some(&isolation) != slot.isolation.as_ref()
            {
                return Err(fail("recorded slot directory or isolation changed"));
            }
            if isolation == "root" {
                if !RELAY_HARNESSES.contains(&record.harness.as_str())
                    || record.adapter != "herdr-relay"
                {
                    return Err(fail("recorded root slot adapter is inconsistent"));
                }
                let mut argv = vec![self
                    .client
                    .harness_executable(&record.harness)?
                    .display()
                    .to_string()];
                argv.extend(arguments.iter().cloned());
                let (line, path, mode) = slot_shell_command(slot, project, &argv)?;
                if fs::canonicalize(path).ok().as_ref() != Some(&cwd) || mode != isolation {
                    return Err(fail("recorded slot policy changed during preflight"));
                }
                relay_executable = Some(argv[0].clone());
                relay_command = Some(line);
            } else {
                if record.adapter == "herdr-relay" {
                    return Err(fail("recorded slot adapter changed"));
                }
                slot_command = Some(line);
            }
        } else if record.adapter == "herdr-relay" {
            return Err(fail("recorded relay has no recoverable slot policy"));
        }
        options.slot = slot;
        let project_workspace = self.start_workspace_policy(&options)?;
        if let Some(owner) =
            self.identity_owner(&native.agent, &native.value, Some(&record.name))?
        {
            return Err(fail(format!(
                "native session is already registered as {:?}; it was {} for {:?}, \
                 so revive will not bind it twice (#928). Name this record's own \
                 session with `agentctl bind-session {} <id>` and revive again",
                owner.name,
                if native.source == "observed" {
                    "discovered, not recorded"
                } else {
                    "recorded"
                },
                record.name,
                record.name
            )));
        }
        Ok(LaunchPlan {
            options,
            arguments,
            native,
            project_workspace,
            slot_command,
            relay_command,
            relay_executable,
        })
    }

    fn revive_liveness(&self, record: &AgentRecord) -> ProcessLiveness {
        let identity = if record.anchors_with_harness_identity() {
            record.harness_anchor()
        } else {
            record.custom_process_identity.as_ref()
        };
        identity.map_or(ProcessLiveness::Unknown, |identity| {
            self.client.process_liveness(identity)
        })
    }

    fn revive_proof(&self, record: &AgentRecord) -> Result<Proof> {
        if self.revive_liveness(record) != ProcessLiveness::Dead {
            return Err(fail(
                "recorded harness process death is unverifiable during recovery proof",
            ));
        }
        let pane = record
            .pane_id
            .as_deref()
            .filter(|value| valid_metadata_text(value))
            .ok_or_else(|| fail("record lacks an owned pane identity"))?;
        let tab = record
            .tab_id
            .as_deref()
            .filter(|value| valid_metadata_text(value))
            .ok_or_else(|| fail("record lacks an owned tab identity"))?;
        let workspace = record
            .workspace_id
            .as_deref()
            .filter(|value| valid_metadata_text(value))
            .ok_or_else(|| fail("record lacks an owned workspace identity"))?;
        let missing = || -> Result<Option<Proof>> {
            match self.client.pane_info(pane) {
                Err(error) if missing_target(&error).is_some() => {
                    let panes = self.client.panes()?;
                    validate_census(&panes)?;
                    if panes
                        .iter()
                        .any(|item| item.pane_id == pane || item.tab_id == tab)
                    {
                        return Err(fail(
                            "missing pane or its recorded tab still appears in the pane census",
                        ));
                    }
                    match self.client.pane_info(pane) {
                        Err(error) if missing_target(&error).is_some() => {
                            let repeated = self.client.panes()?;
                            validate_census(&repeated)?;
                            if repeated
                                .iter()
                                .any(|item| item.pane_id == pane || item.tab_id == tab)
                            {
                                return Err(fail(
                                    "missing pane presentation changed during recovery proof",
                                ));
                            }
                            Ok(Some(Proof {
                                unknown: Default::default(),
                                kind: "missing".to_owned(),
                                pane_id: pane.to_owned(),
                                tab_id: tab.to_owned(),
                                workspace_id: workspace.to_owned(),
                                cwd: record.cwd.clone(),
                                terminal_id: None,
                                reported_agent: None,
                                reported_session_agent: None,
                                reported_session_value: None,
                                shell: None,
                                shell_executable: None,
                            }))
                        }
                        _ => Err(fail("missing pane changed during recovery proof")),
                    }
                }
                Err(error) => Err(error.into()),
                Ok(_) => Ok(None),
            }
        };
        if let Some(proof) = missing()? {
            return Ok(proof);
        }
        let snapshot = || -> Result<(Pane, AgentPaneInfo)> {
            let panes = self.client.panes()?;
            validate_census(&panes)?;
            let presentations: Vec<_> = panes.iter().filter(|item| item.pane_id == pane).collect();
            if presentations.len() != 1
                || presentations[0].tab_id != tab
                || presentations[0].workspace_id != workspace
                || panes.iter().filter(|item| item.tab_id == tab).count() != 1
            {
                return Err(fail(
                    "recorded pane is not the exact owned one-pane presentation",
                ));
            }
            let info = self.client.pane_info(pane)?;
            if info.pane_id != pane
                || info.workspace_id != workspace
                || !same_directory(&info.cwd, &record.cwd)
            {
                return Err(fail("recorded pane, workspace, or cwd changed"));
            }
            let stale_custom = matches!(record.adapter.as_str(), "herdr-pane" | "herdr-relay")
                && record.pane_reported_by_agentctl;
            if info
                .agent
                .as_deref()
                .is_some_and(|agent| !stale_custom || agent != record.harness)
            {
                return Err(fail("recorded pane still reports an unexpected agent"));
            }
            match (&info.session_agent, &info.session_value) {
                (None, None) => {}
                (Some(agent), Some(value))
                    if stale_custom
                        && agent == &record.harness
                        && value == &conversation(record)?.value => {}
                _ => {
                    return Err(fail(
                        "recorded idle pane reports contradictory native session identity",
                    ))
                }
            }
            Ok((presentations[0].clone(), info))
        };
        let before = snapshot()?;
        let shell = self
            .client
            .pane_idle_shell_identity(pane)?
            .ok_or_else(|| fail("owned pane is not a supported descendant-free idle shell"))?;
        let after = snapshot()?;
        if before != after {
            return Err(fail(
                "owned idle presentation changed during recovery proof",
            ));
        }
        if let Some(owner) =
            self.claim_owner(pane, after.1.terminal_id.as_deref(), Some(&record.name))?
        {
            return Err(fail(format!("owned pane is also registered as {owner:?}")));
        }
        let proof = Proof {
            unknown: Default::default(),
            kind: "idle_shell".to_owned(),
            pane_id: pane.to_owned(),
            tab_id: tab.to_owned(),
            workspace_id: workspace.to_owned(),
            cwd: after.1.cwd,
            terminal_id: after.1.terminal_id,
            reported_agent: after.1.agent,
            reported_session_agent: after.1.session_agent,
            reported_session_value: after.1.session_value,
            shell: Some(shell.identity),
            shell_executable: Some(
                shell
                    .executable_path
                    .to_str()
                    .ok_or_else(|| fail("idle shell executable path is not UTF-8"))?
                    .to_owned(),
            ),
        };
        if !proof.valid() {
            return Err(fail("invalid idle shell proof"));
        }
        Ok(proof)
    }

    fn candidate_policy_matches(&self, record: &AgentRecord, old: &AgentRecord) -> Result<bool> {
        let native = revive_conversation(old)?;
        let expected_arguments = expected_arguments(old)?;
        Ok(record.name == old.name
            && record.token != old.token
            && record.harness == old.harness
            && record.paused == old.paused
            && record.goal == old.goal
            && record.name_history == old.name_history
            && record.former_names == old.former_names
            && record.mode == old.mode
            && record.backend == old.backend
            && record.adapter == revived_adapter(old)
            && record.cwd == old.cwd
            && record.profile == old.profile
            && record.model == old.model
            && record.reasoning_effort == old.reasoning_effort
            && record.arguments == expected_arguments
            && record.environment_names == old.environment_names
            && record.slot == old.slot
            && record.slot_project == old.slot_project
            && record.slot_isolation == old.slot_isolation
            && record.extra.get("project_box") == old.extra.get("project_box")
            && record.resume.as_deref() == Some(&native.value)
            && record.native_session.as_ref().is_some_and(|session| {
                session.agent == native.agent && session.value == native.value
            }))
    }

    fn verify_candidate(
        &self,
        record: &AgentRecord,
        old: &AgentRecord,
        proof: &Proof,
    ) -> Result<()> {
        if !self.candidate_policy_matches(record, old)?
            || !matches!(
                record.lifecycle.as_str(),
                "running" | "starting" | "launch_failed"
            )
            || record
                .terminal_id
                .as_deref()
                .is_none_or(|terminal| !valid_metadata_text(terminal))
            || (record.anchors_with_harness_identity() && record.harness_anchor().is_none())
            || (!record.anchors_with_harness_identity() && record.custom_process_identity.is_none())
        {
            return Err(fail(
                "resumed candidate has no independently pinned runtime",
            ));
        }
        let exclusion = CensusExclusion::new(old, proof);
        self.resolve_recovery_target(self.client, &self.target(record)?, Some(&exclusion), None)?;
        let info = self.checked_with_recovery(record, Some(&exclusion))?;
        let mut corroborated = record.clone();
        corroborated.capture_native_session(&info)?;
        if info.session_agent != record.session_agent || info.session_value != record.session_value
        {
            return Err(fail("resumed native session changed after launch"));
        }
        if self
            .claim_owner(
                &info.pane_id,
                info.terminal_id.as_deref(),
                Some(&record.name),
            )?
            .is_some()
        {
            return Err(fail(
                "resumed candidate's runtime is claimed by another record",
            ));
        }
        Ok(())
    }

    fn revive_plan_document(&self, record: &AgentRecord, action: &str, reason: &str) -> Value {
        json!({ "name":record.name, "token":record.token, "harness":record.harness,
            "profile":record.profile, "model":record.model, "cwd":record.cwd, "slot":record.slot,
            "native_session":conversation(record).ok(), "action":action, "reason":reason })
    }

    /// Inspect the same generations that completion will use, without acquiring locks or saving.
    fn pending_revive_plan(&self, journal: &Journal, old: &AgentRecord, stopped: &[u8]) -> Value {
        let inspection = (|| -> Result<()> {
            let active_path = self.directory(&journal.name)?;
            let archive_path = self
                .registry
                .join("archive")
                .join(format!("{}-{}", journal.name, journal.old_token));
            let stage_path = operation_directory(&self.registry, journal).join("new");
            let active = optional_record_at(&active_path, &journal.name)?;
            let stage = optional_record_at(&stage_path, &journal.name)?;
            let published = active
                .as_ref()
                .is_some_and(|(_, record, _)| record.token == journal.new_token);
            if published && stage.is_some() {
                return Err(fail(
                    "published revive still has a conflicting staged generation",
                ));
            }
            if let Some((pinned, _, bytes)) = active
                .as_ref()
                .filter(|(_, record, _)| record.token == journal.old_token)
            {
                if !same_journal_directory(pinned.inode, journal.old_directory_inode)
                    || ![
                        journal.old_record_sha256.as_str(),
                        journal.stopped_record_sha256.as_str(),
                    ]
                    .contains(&digest(bytes).as_str())
                {
                    return Err(fail("old revive generation or record bytes changed"));
                }
                let mut logical: Value = serde_json::from_slice(bytes)
                    .map_err(|_| fail("invalid original revive record"))?;
                logical["lifecycle"] = json!("stopped");
                if logical
                    != serde_json::from_slice::<Value>(stopped)
                        .map_err(|_| fail("invalid prepared stopped record"))?
                {
                    return Err(fail(
                        "prepared stopped record differs from the original generation",
                    ));
                }
                if fs::symlink_metadata(&archive_path).is_ok() {
                    return Err(fail("revive archive destination already exists"));
                }
            } else {
                if active
                    .as_ref()
                    .is_some_and(|(_, record, _)| record.token != journal.new_token)
                {
                    return Err(fail("active agent generation changed during revive"));
                }
                let (pinned, archived, bytes) = read_record_at(&archive_path, &journal.name)?;
                if archived.token != journal.old_token
                    || bytes != stopped
                    || !same_journal_directory(pinned.inode, journal.old_directory_inode)
                {
                    return Err(fail(
                        "revive archive does not contain the exact old generation",
                    ));
                }
            }
            let (candidate_directory, candidate, candidate_bytes) = if published {
                active
                    .as_ref()
                    .ok_or_else(|| fail("published revive record disappeared"))?
            } else {
                stage.as_ref().ok_or_else(|| {
                    fail("retained revive candidate is unavailable; no automatic relaunch")
                })?
            };
            if candidate.token != journal.new_token
                || !same_journal_directory(candidate_directory.inode, journal.new_directory_inode)
            {
                return Err(fail("retained revive candidate generation changed"));
            }
            if journal
                .ready_record_sha256
                .as_ref()
                .is_some_and(|value| value != &digest(candidate_bytes))
            {
                return Err(fail("ready revive record bytes changed"));
            }
            if published {
                if journal.ready_record_sha256.is_none() {
                    return Err(fail("published revive lacks durable ready evidence"));
                }
            } else {
                if journal.ready_record_sha256.is_some() && candidate.lifecycle != "running" {
                    return Err(fail("ready revive candidate lifecycle changed"));
                }
                self.verify_candidate(candidate, old, &journal.proof).map_err(|error| fail(format!(
                    "retained revive candidate cannot be verified: {error}; inspect {} and its recorded terminal; no automatic relaunch; use `agentctl revive {} --dry-run` and `agentctl revive {}`; if the candidate never started, `agentctl revive {} --abandon-candidate` marks it failed and launches again (refused while a harness runs for it)",
                    stage_path.join("agent.json").display(), journal.name, journal.name, journal.name
                )))?;
            }
            let current = self.revive_proof(old)?;
            if current != journal.proof && !(published && current.kind == "missing") {
                return Err(fail("stale revive presentation changed"));
            }
            Ok(())
        })();
        let mut result = match inspection {
            Ok(()) => self.revive_plan_document(
                old,
                "recover",
                "incomplete revive transaction; rerun the same command without another launch",
            ),
            Err(error) => self.revive_plan_document(old, "blocked", &error.to_string()),
        };
        result["transaction_phase"] = json!(journal.phase);
        result
    }

    fn preflight_revive(
        &self,
        record: &AgentRecord,
        timeout: Duration,
    ) -> Result<(LaunchPlan, Proof)> {
        match self.revive_liveness(record) {
            ProcessLiveness::Dead => {}
            ProcessLiveness::Alive => return Err(fail("recorded harness process is alive")),
            ProcessLiveness::Unknown => {
                return Err(fail("recorded harness process death is unverifiable"))
            }
        }
        let proof = self.revive_proof(record)?;
        // Pane reuse (#744): a proved idle shell is relaunched in place. Custom panes
        // are the ones Herdr restores as shells; owned native presentations are
        // already closed and replaced by revive cleanup. A pane that still reports
        // a session is excluded from the census and relaunched elsewhere instead:
        // a candidate on that pane could not be told apart from the stale report.
        let reuse_pane = (proof.kind == "idle_shell"
            && proof.reported_agent.is_none()
            && proof.reported_session_value.is_none()
            && matches!(record.adapter.as_str(), "herdr-pane" | "herdr-relay"))
        .then(|| proof.pane_id.clone());
        let plan = self.revive_launch_plan(record, timeout, reuse_pane)?;
        Ok((plan, proof))
    }

    /// Setup precedes the durable launch latch, so a zero-claim orphan can be preserved safely.
    fn preserve_unlaunched_setup(
        &self,
        old: &AgentRecord,
        operation: &Path,
        source: &PinnedAgentDirectory,
        content: &[u8],
        proof: &Proof,
    ) -> Result<()> {
        match fs::symlink_metadata(operation) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(fail(error.to_string())),
            Ok(_) => {}
        }
        if journals(&self.registry)?
            .iter()
            .any(|journal| journal.old_token == old.token)
        {
            return Err(fail("revive setup acquired a journal before recovery"));
        }
        let parent_path = operation
            .parent()
            .ok_or_else(|| fail("revive setup has no parent"))?;
        let parent = pin_directory(parent_path, &old.name)?;
        let held = pin_directory(operation, &old.name)?;
        if Self::child_directory_identity(&parent.file, &old.token)?
            != Some((held.device, held.inode))
        {
            return Err(fail("orphaned revive setup generation changed"));
        }
        let entries = directory_names(&held)?;
        let mut artifacts = BTreeMap::new();
        for entry in &entries {
            if entry == "new" {
                continue;
            }
            if entry != "stopped.json" && !staging_artifact_name(entry, "stopped.json") {
                return Err(fail(
                    "orphaned revive setup has unexpected contents; inspect it before recovery",
                ));
            }
            artifacts.insert(entry.clone(), artifact_bytes(&held, entry)?);
        }
        if let Some(bytes) = artifacts.get("stopped.json") {
            let stopped: AgentRecord =
                match nested::decode_bytes(bytes, &operation.join("stopped.json")) {
                    Some(record) => record?,
                    None => serde_json::from_slice(bytes)
                        .map_err(|_| fail("invalid orphaned stopped record"))?,
                };
            stopped.validate_loaded(&operation.join("stopped.json"), &old.name)?;
            let mut expected = old.clone();
            expected.lifecycle = "stopped".to_owned();
            if stopped != expected {
                return Err(fail(
                    "orphaned revive setup belongs to a different old generation",
                ));
            }
        }
        let staged = if entries.contains("new") {
            let staged = pin_directory(&operation.join("new"), &old.name)?;
            if Self::child_directory_identity(&held.file, "new")?
                != Some((staged.device, staged.inode))
            {
                return Err(fail("orphaned revive candidate generation changed"));
            }
            let mut contents = BTreeMap::new();
            for entry in directory_names(&staged)? {
                if entry != "agent.json" && !staging_artifact_name(&entry, "agent.json") {
                    return Err(fail("orphaned revive candidate has unexpected contents"));
                }
                contents.insert(entry.clone(), artifact_bytes(&staged, &entry)?);
            }
            if let Some(bytes) = contents.get("agent.json") {
                let candidate: AgentRecord = serde_json::from_slice(bytes)
                    .map_err(|_| fail("invalid orphaned revive candidate"))?;
                candidate.validate_loaded(&staged.path.join("agent.json"), &old.name)?;
                if candidate.lifecycle != "starting"
                    || !self.candidate_policy_matches(&candidate, old)?
                    || candidate.extra.get("revived_from") != Some(&json!(old.token))
                    || candidate.pane_reported_by_agentctl
                    || candidate.workspace_id.is_some()
                    || candidate.tab_id.is_some()
                    || candidate.pane_id.is_some()
                    || candidate.terminal_id.is_some()
                    || candidate.harness_identity.is_some()
                    || candidate.anchor_rule.is_some()
                    || candidate.custom_process_identity.is_some()
                    || candidate.foreign_shell_identity.is_some()
                    || candidate.session_agent.is_some()
                    || candidate.session_value.is_some()
                    || candidate.runtime_home.is_some()
                    || candidate.startup_warning.is_some()
                    || candidate.effective_reasoning_effort.is_some()
                    || candidate.error.is_some()
                    || candidate.goal_delivery.is_some()
                    || candidate.goal_message_id.is_some()
                    || !candidate.goal_messages.is_empty()
                    || candidate.goal_session_id.is_some()
                    || candidate.goal_command.is_some()
                    || candidate
                        .native_session
                        .as_ref()
                        .is_none_or(|native| native.source != "asserted")
                {
                    return Err(fail(
                        "orphaned revive candidate may have been launched; no automatic relaunch",
                    ));
                }
            }
            Some((staged, contents))
        } else {
            None
        };
        if self.record_bytes(source)? != content
            || self.revive_proof(old)? != *proof
            || directory_names(&held)? != entries
        {
            return Err(fail(
                "old generation or orphaned setup changed before recovery",
            ));
        }
        for (entry, bytes) in &artifacts {
            if artifact_bytes(&held, entry)? != *bytes {
                return Err(fail("orphaned revive artifact changed before preservation"));
            }
        }
        if let Some((staged, contents)) = staged.as_ref() {
            if Self::child_directory_identity(&held.file, "new")?
                != Some((staged.device, staged.inode))
                || directory_names(staged)? != contents.keys().cloned().collect()
            {
                return Err(fail(
                    "orphaned revive candidate changed before preservation",
                ));
            }
            for (entry, bytes) in contents {
                if artifact_bytes(staged, entry)? != *bytes {
                    return Err(fail(
                        "orphaned revive candidate bytes changed before preservation",
                    ));
                }
            }
        }
        Self::verify_pinned_agent_directory(&held)?;
        Self::verify_pinned_agent_directory(&parent)?;
        let destination = format!(".orphan-{}-{}", old.token, new_journal_id()?);
        rename_directory_noreplace_at(&parent.file, &old.token, &parent.file, &destination)?;
        parent
            .file
            .sync_all()
            .map_err(|error| fail(error.to_string()))?;
        if Self::child_directory_identity(&parent.file, &destination)?
            != Some((held.device, held.inode))
            || Self::child_directory_identity(&parent.file, &old.token)?.is_some()
        {
            return Err(fail("orphaned revive setup publication changed generation"));
        }
        Self::verify_pinned_agent_directory(&parent)?;
        if self.record_bytes(source)? != content || self.revive_proof(old)? != *proof {
            return Err(fail(
                "old presentation changed while preserving revive setup",
            ));
        }
        Ok(())
    }

    fn write_revive_journal(&self, old: Option<&Journal>, journal: &mut Journal) -> Result<()> {
        let root = pin_directory(&self.registry.join(".revives"), &journal.name)?;
        let file_name = format!("{}.json", journal.old_token);
        match old {
            Some(expected) if artifact_bytes(&root, &file_name)? != expected.expected_bytes => {
                return Err(fail("revive journal changed before update"));
            }
            None if fs::symlink_metadata(root.path.join(&file_name)).is_ok() => {
                return Err(fail("revive journal already exists"));
            }
            _ => {}
        }
        let content = document_bytes(journal)?;
        atomic_replace_bytes(&root, &file_name, &content).map_err(|error| *error.error)?;
        journal.expected_bytes = content;
        Ok(())
    }

    /// Revive a record `stop` archived (#741): restore the newest `archive/<name>-<token>`
    /// directory as the live record, mark it running, then revive it. A live record wins.
    pub fn revive_archived(
        &self,
        agent_name: &str,
        dry_run: bool,
        expected_token: Option<&str>,
        startup_timeout: Duration,
    ) -> Result<Value> {
        if self.read_record(agent_name).is_ok() {
            return self.revive(agent_name, dry_run, expected_token, startup_timeout);
        }
        let archive = self.registry.join("archive");
        let archived = newest_archive(&archive, agent_name).ok_or_else(|| {
            fail(format!(
                "unknown agent {agent_name:?}; no archived record either"
            ))
        })?;
        if dry_run {
            return Ok(
                json!({"name": agent_name, "archived": archived, "action": "revive",
                "detail": "archived record would be restored and revived"}),
            );
        }
        let destination = self.directory(agent_name)?;
        fs::rename(&archived, &destination).map_err(|e| fail(e.to_string()))?;
        let mut record = self.read_record(agent_name)?;
        record.lifecycle = "running".to_owned();
        self.save(&record)?;
        self.revive(agent_name, dry_run, expected_token, startup_timeout)
    }

    /// Plan or recover one dead, owned interactive generation without replaying its queue.
    pub fn revive(
        &self,
        agent_name: &str,
        dry_run: bool,
        expected_token: Option<&str>,
        startup_timeout: Duration,
    ) -> Result<Value> {
        self.revive_with(agent_name, dry_run, expected_token, startup_timeout, false)
    }

    /// [`Self::revive`], and with `abandon_candidate` (`revive --abandon-candidate`, #899) an
    /// interrupted revive whose candidate cannot be proved alive or dead is marked failed too,
    /// unless a harness process runs for it. A candidate proved dead is abandoned either way.
    ///
    /// A revive archives the old queue and never replays it, but no prompt goes silently: the
    /// result's `archived_prompts` names every one it left undelivered, and each is recorded as
    /// failed in the new generation's queue ([`Self::report_archived_prompts`]).
    pub fn revive_with(
        &self,
        agent_name: &str,
        dry_run: bool,
        expected_token: Option<&str>,
        startup_timeout: Duration,
        abandon_candidate: bool,
    ) -> Result<Value> {
        let mut result = self.revive_once(
            agent_name,
            dry_run,
            expected_token,
            startup_timeout,
            abandon_candidate,
        )?;
        if let Some(archive) = result["archive"].as_str().filter(|_| !dry_run) {
            match self.report_archived_prompts(agent_name, Path::new(archive)) {
                Ok(reported) => result["archived_prompts"] = reported,
                Err(error) => result["archived_prompts_error"] = json!(error.to_string()),
            }
        }
        Ok(result)
    }

    fn revive_once(
        &self,
        agent_name: &str,
        dry_run: bool,
        expected_token: Option<&str>,
        startup_timeout: Duration,
        abandon_candidate: bool,
    ) -> Result<Value> {
        name(agent_name)?;
        if startup_timeout.is_zero() || startup_timeout > Duration::from_secs(300) {
            return Err(fail("startup timeout must be between 0 and 300 seconds"));
        }
        if expected_token.is_some_and(|value| !token(value)) {
            return Err(fail("expected-token is invalid"));
        }
        agent::validate_private_directory(&self.registry, "agent registry", false)?;
        let pending = journals(&self.registry)?
            .into_iter()
            .find(|journal| journal.name == agent_name);
        if dry_run {
            if let Some(journal) = pending {
                if expected_token.is_some_and(|value| value != journal.old_token) {
                    return Err(fail("agent was replaced before revive"));
                }
                let (mut old, stopped) = stopped_artifact(&self.registry, &journal)?;
                old.lifecycle = "running".to_owned();
                if let Some(check) = self.abandonable(&journal)? {
                    let mut plan = match (check, abandon_candidate) {
                        (CandidateCheck::Dead(evidence), _)
                        | (CandidateCheck::Unproven(evidence), true) => self.revive_plan_document(
                            &old,
                            "revive",
                            &format!(
                                "the interrupted revive's candidate is abandoned ({evidence}): \
                                 the attempt is kept as evidence and the agent launches again"
                            ),
                        ),
                        (CandidateCheck::Live(evidence), true) => self.revive_plan_document(
                            &old,
                            "blocked",
                            &format!(
                                "the interrupted revive's candidate is not abandoned: {evidence}"
                            ),
                        ),
                        (CandidateCheck::Unproven(evidence), false) => {
                            match self.failed_candidate_pane(&journal)? {
                                Some(pane) => self.revive_plan_document(
                                    &old,
                                    "revive",
                                    &format!(
                                        "the interrupted revive's candidate is abandoned \
                                         ({evidence}; its launch failed in pane {pane}, which \
                                         this revive opened and closes): the attempt is kept as \
                                         evidence and the agent launches again"
                                    ),
                                ),
                                None => self.pending_revive_plan(&journal, &old, &stopped),
                            }
                        }
                        _ => self.pending_revive_plan(&journal, &old, &stopped),
                    };
                    plan["transaction_phase"] = json!(journal.phase);
                    return Ok(plan);
                }
                if abandon_candidate {
                    return Err(fail(format!(
                        "the interrupted revive of {agent_name:?} is past launching ({}); it has \
                         no candidate to abandon",
                        journal.phase
                    )));
                }
                return Ok(self.pending_revive_plan(&journal, &old, &stopped));
            }
            if abandon_candidate {
                return Err(fail(format!(
                    "agent {agent_name:?} has no interrupted revive candidate to abandon"
                )));
            }
            let record = self.read_record(agent_name)?;
            if expected_token.is_some_and(|value| value != record.token) {
                return Err(fail("agent was replaced before revive"));
            }
            if record.lifecycle != "running" {
                return Ok(self.revive_plan_document(
                    &record,
                    "skip",
                    "record lifecycle is not running",
                ));
            }
            if self.revive_liveness(&record) == ProcessLiveness::Alive {
                return Ok(self.revive_plan_document(
                    &record,
                    "skip",
                    "recorded harness process is alive",
                ));
            }
            return Ok(match self.preflight_revive(&record, startup_timeout) {
                Ok(_) => self.revive_plan_document(
                    &record,
                    "revive",
                    "recorded harness is dead and recovery policy is verified",
                ),
                Err(error) => self.revive_plan_document(&record, "blocked", &error.to_string()),
            });
        }
        let _name = self.lock(agent_name)?;
        let _identity = self.identity_lock()?;
        let pending = journals(&self.registry)?
            .into_iter()
            .find(|journal| journal.name == agent_name);
        if let Some(mut journal) = pending {
            if expected_token.is_some_and(|value| value != journal.old_token) {
                return Err(fail("agent was replaced before revive"));
            }
            let (old, _) = stopped_artifact(&self.registry, &journal)?;
            let old_path = self.directory(agent_name)?;
            let old_path = if fs::symlink_metadata(&old_path).is_ok()
                && self.read_record(agent_name)?.token == journal.old_token
            {
                old_path
            } else {
                self.registry
                    .join("archive")
                    .join(format!("{}-{}", journal.name, journal.old_token))
            };
            let queue = queue_locks_at(&old_path)?;
            let pane = self.pane_lock(&journal.proof.pane_id)?;
            let abandoned = match self.abandonable(&journal)? {
                Some(CandidateCheck::Dead(evidence)) => {
                    Some(self.abandon_revive(&journal, &evidence, abandon_candidate)?)
                }
                Some(CandidateCheck::Unproven(evidence)) if abandon_candidate => {
                    Some(self.abandon_revive(&journal, &evidence, true)?)
                }
                Some(CandidateCheck::Unproven(evidence)) => {
                    match self.close_failed_candidate_pane(&journal)? {
                        Some(closed) => Some(self.abandon_revive(
                            &journal,
                            &format!("{evidence}; {closed}"),
                            false,
                        )?),
                        None => None,
                    }
                }
                Some(CandidateCheck::Live(evidence)) if abandon_candidate => {
                    return Err(fail(format!(
                        "the interrupted revive candidate of {agent_name:?} is not abandoned: \
                         {evidence}; `agentctl revive {agent_name}` verifies and publishes a live \
                         candidate"
                    )));
                }
                None if abandon_candidate => {
                    return Err(fail(format!(
                        "the interrupted revive of {agent_name:?} is past launching ({}); it has \
                         no candidate to abandon",
                        journal.phase
                    )));
                }
                _ => None,
            };
            let Some(abandoned) = abandoned else {
                return self.finish_revive(&mut journal, &old);
            };
            // The attempt is out of the way: revive again from the start, under fresh locks.
            drop((pane, queue, _identity, _name));
            let mut result =
                self.revive_once(agent_name, false, expected_token, startup_timeout, false)?;
            result["abandoned_candidate"] = abandoned;
            return Ok(result);
        }
        if abandon_candidate {
            return Err(fail(format!(
                "agent {agent_name:?} has no interrupted revive candidate to abandon"
            )));
        }
        let record = self.read_record(agent_name)?;
        if expected_token.is_some_and(|value| value != record.token) {
            return Err(fail("agent was replaced before revive"));
        }
        let (plan, proof) = self.preflight_revive(&record, startup_timeout)?;
        let pinned = self.pinned_agent_directory(agent_name)?;
        let original = self.record_bytes(&pinned)?;
        let snapshot: AgentRecord =
            match nested::decode_bytes(&original, &pinned.path.join("agent.json")) {
                Some(record) => record?,
                None => serde_json::from_slice(&original)
                    .map_err(|_| fail("invalid revive source record"))?,
            };
        if snapshot != record {
            return Err(fail("revive source record changed during preflight"));
        }
        let _queue = self.queue_locks(agent_name)?;
        let _pane = self.pane_lock(&proof.pane_id)?;
        if self.record_bytes(&pinned)? != original
            || self.revive_liveness(&record) != ProcessLiveness::Dead
            || self.revive_proof(&record)? != proof
        {
            return Err(fail(
                "revive source generation or runtime changed before launch",
            ));
        }
        let destination = self
            .registry
            .join("archive")
            .join(format!("{}-{}", record.name, record.token));
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(fail("revive archive already exists"));
        }
        let root = self.registry.join(".revives");
        agent::create_private_directory(&root, "revive journal directory", false, false)?;
        agent::sync_directory(&self.registry)?;
        let operation = root.join(&record.token);
        self.preserve_unlaunched_setup(&record, &operation, &pinned, &original, &proof)?;
        DirBuilder::new()
            .mode(0o700)
            .create(&operation)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&root)?;
        checkpoint("operation-created")?;
        let new_path = operation.join("new");
        DirBuilder::new()
            .mode(0o700)
            .create(&new_path)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&operation)?;
        checkpoint("stage-created")?;
        let new_directory = pin_directory(&new_path, agent_name)?;
        let operation_pinned = pin_directory(&operation, agent_name)?;
        let mut stopped: Value =
            serde_json::from_slice(&original).map_err(|_| fail("invalid revive source record"))?;
        stopped["lifecycle"] = json!("stopped");
        let stopped_bytes = document_bytes(&stopped)?;
        atomic_replace_bytes(&operation_pinned, "stopped.json", &stopped_bytes)
            .map_err(|error| *error.error)?;
        checkpoint("stopped-prepared")?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_secs_f64();
        let new_token = new_journal_id()?;
        let mut candidate = record.clone();
        candidate.adapter = revived_adapter(&record).to_owned();
        // The new generation is a flat schema-1 record whatever form the old one had.
        candidate.nested = None;
        // Edges naming the old generation still reach the new one, and its own edges survive.
        candidate.succeed(&record, self.token_referenced(&record.token));
        candidate.storage_directory = Some(StorageDirectory {
            path: new_path,
            device: new_directory.device,
            inode: new_directory.inode,
        });
        candidate.token = new_token.clone();
        candidate.created_at = now;
        candidate.lifecycle = "starting".to_owned();
        candidate.workspace_id = None;
        candidate.tab_id = None;
        candidate.pane_id = None;
        candidate.terminal_id = None;
        candidate.harness_identity = None;
        candidate.anchor_rule = None;
        candidate.custom_process_identity = None;
        candidate.foreign_shell_identity = None;
        candidate.session_agent = None;
        candidate.session_value = None;
        candidate.runtime_home = None;
        candidate.pane_reported_by_agentctl = false;
        candidate.startup_warning = None;
        candidate.effective_reasoning_effort = None;
        candidate.error = None;
        candidate.goal_delivery = None;
        candidate.goal_session_id = None;
        candidate.goal_command = None;
        candidate.goal_messages.clear();
        candidate.goal_message_id = None;
        candidate.resume = Some(plan.native.value.clone());
        candidate.native_session = Some(NativeSession::new(
            &plan.native.agent,
            &plan.native.value,
            "asserted",
        ));
        candidate.arguments = plan.arguments;
        candidate
            .extra
            .insert("revived_from".to_owned(), json!(record.token));
        // The fresh conversation is this generation's own; its next revive resumes it.
        candidate.extra.remove(FRESH_REVIVE);
        let previous_label = record
            .launch_spec
            .as_ref()
            .and_then(|spec| spec["workspace_label"].as_str().map(str::to_owned));
        candidate.launch_spec = Some(launch_spec::build(launch_spec::SpecFields {
            source: "revive",
            captured_at: now,
            harness: &candidate.harness,
            executable: plan.relay_executable.as_deref(),
            argv: &candidate.arguments,
            cwd: &candidate.cwd,
            profile: candidate.profile.as_deref(),
            environment_names: &candidate.environment_names,
            slot: candidate.slot.as_deref(),
            slot_project: candidate.slot_project.as_deref(),
            slot_isolation: candidate.slot_isolation.as_deref(),
            workspace_label: previous_label.as_deref(),
            native_session: Some((&plan.native.value, "asserted")),
        })?);
        self.save(&candidate)?;
        checkpoint("candidate-prepared")?;
        let mut journal = Journal {
            unknown: Default::default(),
            expected_bytes: Vec::new(),
            schema: SCHEMA.to_owned(),
            name: agent_name.to_owned(),
            old_token: record.token.clone(),
            new_token,
            phase: "launching".to_owned(),
            started_at: now,
            old_directory_device: pinned.device,
            old_directory_inode: pinned.inode,
            new_directory_device: new_directory.device,
            new_directory_inode: new_directory.inode,
            old_record_sha256: digest(&original),
            stopped_record_sha256: digest(&stopped_bytes),
            ready_record_sha256: None,
            proof,
        };
        self.write_revive_journal(None, &mut journal)?;
        checkpoint("launching")?;
        let exclusion = CensusExclusion::new(&record, &journal.proof);
        if let Err(error) = self.launch(
            &mut candidate,
            &plan.options,
            plan.project_workspace.as_deref(),
            plan.slot_command.as_deref(),
            plan.relay_command.as_deref(),
            Some(&exclusion),
        ) {
            candidate.lifecycle = "launch_failed".to_owned();
            candidate.error = Some(if plan.options.environment.is_empty() {
                error.to_string()
            } else {
                "launch failed with profile environment; details omitted from status".to_owned()
            });
            self.save(&candidate)?;
            return Err(fail(format!("revive launch of {agent_name:?} is incomplete; staged artifacts and journal retained; inspect the candidate before rerunning `agentctl revive {agent_name}`")));
        }
        checkpoint("launched")?;
        self.verify_candidate(&candidate, &record, &journal.proof)?;
        // Pane reuse (#744): the candidate already relaunched in the old pane,
        // so the idle-shell proof cannot be re-derived; it was proved at launch.
        let reused = candidate.pane_id.as_deref() == Some(journal.proof.pane_id.as_str());
        if self.record_bytes(&pinned)? != original
            || (!reused && self.revive_proof(&record)? != journal.proof)
        {
            return Err(fail(
                "revive source changed after candidate launch; journal retained",
            ));
        }
        let before = journal.clone();
        journal.ready_record_sha256 = Some(digest(&self.record_bytes(&new_directory)?));
        journal.phase = "ready".to_owned();
        self.write_revive_journal(Some(&before), &mut journal)?;
        checkpoint("ready")?;
        self.finish_revive(&mut journal, &record)
    }

    /// Name and record the prompts a revive archived without delivering them (#899): every item
    /// left in the archived queue's inbox, in flight, or failed. Each is copied into the new
    /// generation's `failed/` with the reason, unless it is already there, so a sender asking
    /// after it reads failed, never delivered, the failed-queue digest names it, and no drain
    /// ever types it. Returns one row per prompt: its ID, sender, first line and old lane.
    fn report_archived_prompts(&self, agent_name: &str, archive: &Path) -> Result<Value> {
        use agent_queue::queue::QueueDirectories;
        let old = QueueDirectories::new(&archive.join("queue"));
        let failed = QueueDirectories::new(&self.queue(agent_name)?).failed;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_secs_f64();
        let mut reported = Vec::new();
        for (lane, directory) in [
            ("inbox", &old.inbox),
            ("inbox.v2", &old.inbox_v2),
            ("inflight", &old.inflight),
            ("failed", &old.failed),
        ] {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "json")
                })
                .collect();
            paths.sort();
            for path in paths {
                let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let Ok(document) = agent_queue::queue::load_message(&path) else {
                    continue;
                };
                if id.starts_with(crate::failed_digest::DIGEST_ID_PREFIX)
                    || document.get(crate::failed_digest::DIGEST_FLAG) == Some(&Value::Bool(true))
                {
                    continue;
                }
                let text = document.get("text").and_then(Value::as_str).unwrap_or("");
                let target = failed.join(format!("{id}.json"));
                if !target.exists() {
                    agent::create_private_directory(
                        &failed,
                        "revived queue failed lane",
                        true,
                        false,
                    )?;
                    agent::atomic_json(&target, &Value::Object(document.clone()))?;
                    agent::atomic_json(
                        &PathBuf::from(format!("{}.error", target.display())),
                        &json!({
                            "error": format!(
                                "archived by revive, not delivered: the old generation's queue \
                                 was archived and is never replayed; the prompt is kept at {}",
                                path.display()
                            ),
                            "failed_at": now,
                        }),
                    )?;
                }
                reported.push(json!({
                    "id": id,
                    "sender": crate::failed_digest::sender_of(&document, text),
                    "first_line": crate::failed_digest::first_line_of(text),
                    "lane": lane,
                }));
            }
        }
        Ok(json!(reported))
    }

    /// The independent check of `journal`'s candidate (#899), or `None` when the attempt is past
    /// launching or its old generation is no longer the active record: only a revive interrupted
    /// while launching, before anything was published, can be abandoned.
    fn abandonable(&self, journal: &Journal) -> Result<Option<CandidateCheck>> {
        if journal.phase != "launching" || journal.ready_record_sha256.is_some() {
            return Ok(None);
        }
        let active = optional_record_at(&self.directory(&journal.name)?, &journal.name)?;
        if !active.is_some_and(|(pinned, record, _)| {
            record.token == journal.old_token
                && same_journal_directory(pinned.inode, journal.old_directory_inode)
        }) {
            return Ok(None);
        }
        let stage_path = operation_directory(&self.registry, journal).join("new");
        let (pinned, candidate, _) = read_record_at(&stage_path, &journal.name)?;
        if candidate.token != journal.new_token
            || !same_journal_directory(pinned.inode, journal.new_directory_inode)
        {
            return Err(fail("staged revive generation changed"));
        }
        self.check_candidate(&candidate).map(Some)
    }

    /// Whether a harness can be running for `candidate`: its pinned process alive, or Herdr
    /// seeing an agent in its pane, is live; its pane gone, or holding only an idle shell with
    /// no child process, is dead; anything else is unproven, including a candidate that recorded
    /// no pane, since a harness it started before recording one cannot be checked.
    fn check_candidate(&self, candidate: &AgentRecord) -> Result<CandidateCheck> {
        let identity = if candidate.anchors_with_harness_identity() {
            candidate.harness_anchor()
        } else {
            candidate.custom_process_identity.as_ref()
        };
        if let Some(identity) = identity {
            if self.client.process_liveness(identity) == ProcessLiveness::Alive {
                return Ok(CandidateCheck::Live(format!(
                    "its {} process {} is alive",
                    candidate.harness, identity.pid
                )));
            }
        }
        let Some(pane) = candidate.pane_id.as_deref() else {
            return Ok(CandidateCheck::Unproven(
                "the candidate recorded no pane, so a harness it may have started cannot be \
                 checked"
                    .to_owned(),
            ));
        };
        if !self.client.panes()?.iter().any(|row| row.pane_id == pane) {
            return Ok(CandidateCheck::Dead(format!("its pane {pane} is gone")));
        }
        if let Some(kind) = self.client.pane_info(pane).ok().and_then(|info| info.agent) {
            return Ok(CandidateCheck::Live(format!(
                "Herdr reports {kind} running in its pane {pane}"
            )));
        }
        if self.client.pane_is_idle_shell(pane)? {
            return Ok(CandidateCheck::Dead(format!(
                "its pane {pane} holds only an idle shell with no child process, so no harness \
                 runs there; the pane is left open"
            )));
        }
        Ok(CandidateCheck::Unproven(format!(
            "its pane {pane} runs a process agentctl cannot identify as a harness or an idle shell"
        )))
    }

    /// Close the pane of an unproven candidate whose launch failed, when this revive opened that
    /// pane itself (#920 revive-repeat-incomplete): the launch reported that its harness never
    /// came up, no pinned harness process is alive and Herdr sees no agent there, so whatever
    /// still runs in the pane (a wrapper that never started the harness, say) is this attempt's
    /// own. Closing the pane ends it, which makes the attempt as dead as #899's dead candidates.
    /// A candidate relaunched in the old pane is never closed: that pane is the old
    /// generation's. Returns the evidence when it closed the pane.
    fn close_failed_candidate_pane(&self, journal: &Journal) -> Result<Option<String>> {
        let Some(pane) = self.failed_candidate_pane(journal)? else {
            return Ok(None);
        };
        self.client.close_pane(&pane)?;
        Ok(Some(format!(
            "its launch failed and its harness never came up in pane {pane}, which this revive \
             opened, so that pane was closed"
        )))
    }

    /// The pane [`Self::close_failed_candidate_pane`] would close, read-only.
    fn failed_candidate_pane(&self, journal: &Journal) -> Result<Option<String>> {
        let stage_path = operation_directory(&self.registry, journal).join("new");
        let (_, candidate, _) = read_record_at(&stage_path, &journal.name)?;
        let Some(pane) = candidate.pane_id.as_deref() else {
            return Ok(None);
        };
        if candidate.lifecycle != "launch_failed" || pane == journal.proof.pane_id {
            return Ok(None);
        }
        let opened_here = self.client.panes()?.iter().any(|row| {
            row.pane_id == pane
                && Some(&row.tab_id) == candidate.tab_id.as_ref()
                && Some(&row.workspace_id) == candidate.workspace_id.as_ref()
        });
        Ok(opened_here.then(|| pane.to_owned()))
    }

    /// Mark `journal`'s attempt failed (#899): its journal and operation directory move, whole,
    /// into `.revives/.abandoned/TOKEN-SECONDS/` beside `abandoned.json`, which records the
    /// evidence and whether the operator asked (`explicit`). The old generation and its queue
    /// are untouched, so the next revive launches as if the attempt had never begun.
    fn abandon_revive(&self, journal: &Journal, evidence: &str, explicit: bool) -> Result<Value> {
        let root = self.registry.join(".revives");
        let parent = root.join(ABANDONED);
        agent::create_private_directory(&parent, "abandoned revive directory", false, false)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?;
        let destination = parent.join(format!("{}-{}", journal.old_token, now.as_secs()));
        agent::create_private_directory(&destination, "abandoned revive attempt", false, false)?;
        let mut record = json!({
            "name": journal.name,
            "old_token": journal.old_token,
            "new_token": journal.new_token,
            "phase": journal.phase,
            "started_at": journal.started_at,
            "abandoned_at": now.as_secs_f64(),
            "explicit": explicit,
            "evidence": evidence,
        });
        agent::atomic_json(&destination.join("abandoned.json"), &record)?;
        let moved = |from: PathBuf, to: PathBuf| {
            fs::rename(&from, &to).map_err(|error| {
                fail(format!(
                    "cannot move {} into {}: {error}",
                    from.display(),
                    to.display()
                ))
            })
        };
        // The journal goes first: if the second move fails, the operation directory is left as
        // orphaned set-up, which the next revive inspects before it launches.
        moved(
            root.join(format!("{}.json", journal.old_token)),
            destination.join("journal.json"),
        )?;
        moved(
            operation_directory(&self.registry, journal),
            destination.join("operation"),
        )?;
        agent::sync_directory(&destination)?;
        agent::sync_directory(&root)?;
        record["kept_at"] = json!(destination);
        Ok(record)
    }

    /// Anchor a launch-failed revive candidate whose harness still runs in the candidate's own
    /// pane, as `launch` anchors one that passed its readiness check: the pinned custom process
    /// must still be the pane's foreground program, or a native harness Herdr reports there is
    /// pinned now. Returns whether the candidate is anchored; nothing changes otherwise.
    fn reanchor_failed_candidate(&self, candidate: &mut AgentRecord) -> Result<bool> {
        let Some(pane) = candidate.pane_id.clone() else {
            return Ok(false);
        };
        let Ok(info) = self.client.pane_info(&pane) else {
            return Ok(false);
        };
        if Some(&info.workspace_id) != candidate.workspace_id.as_ref() || info.terminal_id.is_none()
        {
            return Ok(false);
        }
        // The conversation the pane reports, as `launch` records it, and only the requested one.
        let mut reported = candidate.clone();
        if reported.capture_native_session(&info).is_err() {
            return Ok(false);
        }
        reported.session_agent.clone_from(&info.session_agent);
        reported.session_value.clone_from(&info.session_value);
        if candidate.anchors_with_harness_identity() {
            if info.agent.as_deref() != Some(candidate.harness.as_str()) {
                return Ok(false);
            }
            let mut anchored = reported;
            if self.anchor_fresh(&mut anchored, info.terminal_id).is_err()
                || anchored.harness_anchor().is_none()
            {
                return Ok(false);
            }
            *candidate = anchored;
            return Ok(true);
        }
        let Some(identity) = candidate.custom_process_identity.clone() else {
            return Ok(false);
        };
        if self
            .client
            .verify_custom_harness(&pane, &candidate.harness, Some(&identity))
            .is_err()
        {
            return Ok(false);
        }
        reported.terminal_id = info.terminal_id;
        *candidate = reported;
        Ok(true)
    }

    fn finish_revive(&self, journal: &mut Journal, old: &AgentRecord) -> Result<Value> {
        let (_, stopped) = stopped_artifact(&self.registry, journal)?;
        let mut active_path = self.directory(&journal.name)?;
        let archive_path = self
            .registry
            .join("archive")
            .join(format!("{}-{}", journal.name, journal.old_token));
        let stage_path = operation_directory(&self.registry, journal).join("new");
        let active = match fs::symlink_metadata(&active_path) {
            Ok(_) => Some(read_record_at(&active_path, &journal.name)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(fail(error.to_string())),
        };
        let already_published = active
            .as_ref()
            .is_some_and(|(_, record, _)| record.token == journal.new_token);
        if already_published {
            match fs::symlink_metadata(&stage_path) {
                Ok(_) => return Err(fail(
                    "published revive still has a conflicting staged generation; journal retained",
                )),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(fail(error.to_string())),
            }
        }
        if !already_published {
            let (staged, mut candidate, mut staged_bytes) =
                read_record_at(&stage_path, &journal.name)?;
            if !same_journal_directory(staged.inode, journal.new_directory_inode)
                || candidate.token != journal.new_token
            {
                return Err(fail("staged revive generation changed"));
            }
            // A launch whose readiness check failed after its harness started (#920: a Muse
            // idle at its editor whose header had scrolled off) leaves a candidate with no
            // terminal anchor, so it could never be verified. When the harness still runs in
            // the candidate's own pane, anchor it now as the launch would have.
            if candidate.lifecycle == "launch_failed"
                && candidate.terminal_id.is_none()
                && self.reanchor_failed_candidate(&mut candidate)?
            {
                candidate.storage_directory = Some(StorageDirectory {
                    path: stage_path.clone(),
                    device: staged.device,
                    inode: staged.inode,
                });
                self.save(&candidate)?;
                staged_bytes = artifact_bytes(&staged, "agent.json")?;
            }
            self.verify_candidate(&candidate, old, &journal.proof).map_err(|_| fail(format!(
                "interrupted revive candidate is not independently verified; refusing another launch; no automatic relaunch; journal and diagnostics retained at {}; use `agentctl revive {} --dry-run` and `agentctl revive {}`; if the candidate never started, `agentctl revive {} --abandon-candidate` marks it failed and launches again (refused while a harness runs for it)",
                stage_path.join("agent.json").display(), journal.name, journal.name, journal.name
            )))?;
            if candidate.lifecycle != "running" {
                if journal.ready_record_sha256.is_some() {
                    return Err(fail(
                        "ready revive candidate lifecycle changed; journal retained",
                    ));
                }
                candidate.lifecycle = "running".to_owned();
                candidate.error = None;
                candidate.storage_directory = Some(StorageDirectory {
                    path: stage_path.clone(),
                    device: staged.device,
                    inode: staged.inode,
                });
                self.save(&candidate)?;
                staged_bytes = artifact_bytes(&staged, "agent.json")?;
            }
            if journal.ready_record_sha256.is_none() {
                let before = journal.clone();
                journal.ready_record_sha256 = Some(digest(&staged_bytes));
                journal.phase = "ready".to_owned();
                self.write_revive_journal(Some(&before), journal)?;
            }
            if journal.ready_record_sha256.as_deref() != Some(&digest(&staged_bytes)) {
                return Err(fail("verified revive candidate record changed"));
            }
            if let Some((pinned, record, bytes)) = active {
                if record.token != journal.old_token
                    || !same_journal_directory(pinned.inode, journal.old_directory_inode)
                {
                    return Err(fail(
                        "active agent generation changed before revive publication",
                    ));
                }
                let hash = digest(&bytes);
                if hash != journal.old_record_sha256 && hash != journal.stopped_record_sha256 {
                    return Err(fail("original revive record changed before retirement"));
                }
                if hash == journal.old_record_sha256 {
                    let mut logical: Value = serde_json::from_slice(&bytes)
                        .map_err(|_| fail("invalid original revive record"))?;
                    logical["lifecycle"] = json!("stopped");
                    if logical
                        != serde_json::from_slice::<Value>(&stopped)
                            .map_err(|_| fail("invalid stopped revive record"))?
                    {
                        return Err(fail(
                            "prepared stopped record differs from original recovery metadata",
                        ));
                    }
                }
                // Pane reuse (#744): the candidate already relaunched in this pane,
                // so the idle-shell proof cannot be re-derived; it was proved at launch.
                let reused = candidate.pane_id.as_deref() == Some(journal.proof.pane_id.as_str());
                if !reused && self.revive_proof(old)? != journal.proof {
                    return Err(fail("original idle shell proof changed before retirement"));
                }
                if journal.proof.kind == "idle_shell" && !reused {
                    let text = self.bounded_terminal_text(&journal.proof.pane_id)?;
                    let output = document_bytes_with_limit(
                        &json!({ "text":text, "captured_at":journal.started_at,
                            "pane_id":journal.proof.pane_id, "retirement_shell_identity":journal.proof.shell,
                        }),
                        MAX_SNAPSHOT_BYTES,
                    )?;
                    atomic_replace_bytes(&pinned, "output.json", &output)
                        .map_err(|error| *error.error)?;
                }
                if self.record_bytes(&pinned)? != bytes
                    || (!reused && self.revive_proof(old)? != journal.proof)
                {
                    return Err(fail("revive source changed immediately before retirement"));
                }
                atomic_replace_bytes(&pinned, "agent.json", &stopped)
                    .map_err(|error| *error.error)?;
                checkpoint("stopped")?;
                agent::create_private_directory(
                    &self.registry.join("archive"),
                    "agent archive",
                    false,
                    false,
                )?;
                self.publish_pinned_directory(&pinned, &archive_path, &stopped)?;
                checkpoint("archived")?;
            } else {
                let (archived, record, bytes) = read_record_at(&archive_path, &journal.name)?;
                if record.token != journal.old_token
                    || bytes != stopped
                    || !same_journal_directory(archived.inode, journal.old_directory_inode)
                {
                    return Err(fail(
                        "revive archive does not contain the proved old generation",
                    ));
                }
            }
            self.verify_candidate(&candidate, old, &journal.proof)?;
            // The old generation has left its directory, so the new one is published in layout 2
            // whichever layout the old one used (#593 registry-agents-subdir).
            self.prepare_layout()?;
            active_path = self.directory(&journal.name)?;
            publish_candidate(&staged, &active_path, &staged_bytes)?;
            checkpoint("published")?;
        }
        let (active, mut candidate, bytes) = read_record_at(&active_path, &journal.name)?;
        if candidate.token != journal.new_token
            || !same_journal_directory(active.inode, journal.new_directory_inode)
            || journal.ready_record_sha256.as_deref() != Some(&digest(&bytes))
        {
            return Err(fail("published revive generation changed"));
        }
        let (archived, archived_record, archive_bytes) =
            read_record_at(&archive_path, &journal.name)?;
        if archived_record.token != journal.old_token
            || archive_bytes != stopped
            || !same_journal_directory(archived.inode, journal.old_directory_inode)
        {
            return Err(fail("published revive has no exact stopped archive"));
        }
        if journal.phase != "published" {
            let before = journal.clone();
            journal.phase = "published".to_owned();
            self.write_revive_journal(Some(&before), journal)?;
        }
        // Pane reuse (#744): the candidate relaunched in the old idle shell, so the
        // pane and tab stay; the old presentation proof no longer describes them.
        let reused = candidate.pane_id.as_deref() == Some(journal.proof.pane_id.as_str());
        let current = if reused {
            journal.proof.clone()
        } else {
            self.revive_proof(old)?
        };
        // An adopted agent's pane was never agentctl's to close, even as an idle shell.
        let keep_pane = reused || (old.is_foreign() && current.kind != "missing");
        if current.kind != "missing" && !keep_pane {
            if current != journal.proof {
                return Err(fail(
                    "stale pane shell generation changed; revive cleanup journal retained",
                ));
            }
            self.client.close_pane(&journal.proof.pane_id).map_err(|_| fail(format!("revive published but stale pane close is uncertain; rerun `agentctl revive {} --expected-token {}`", journal.name, journal.old_token)))?;
            checkpoint("closed")?;
            if self.revive_proof(old)?.kind != "missing" {
                return Err(fail(
                    "stale pane closure is not verified; revive journal retained",
                ));
            }
        }
        let panes = self.client.panes()?;
        validate_census(&panes)?;
        let tab_closed = !panes.iter().any(|pane| pane.tab_id == journal.proof.tab_id);
        if !tab_closed && !keep_pane {
            return Err(fail(
                "stale revive tab reappeared during cleanup; journal retained",
            ));
        }
        if artifact_bytes(&active, "agent.json")? != bytes {
            return Err(fail("published revive record changed during cleanup"));
        }
        if artifact_bytes(&archived, "agent.json")? != stopped {
            return Err(fail(
                "old revive archive changed during cleanup; journal retained",
            ));
        }
        let root = pin_directory(&self.registry.join(".revives"), &journal.name)?;
        if artifact_bytes(&root, &format!("{}.json", journal.old_token))? != journal.expected_bytes
        {
            return Err(fail("revive journal changed before cleanup completion"));
        }
        let file_name = CString::new(format!("{}.json", journal.old_token))
            .map_err(|_| fail("invalid revive journal name"))?;
        if unsafe { libc::unlinkat(root.file.as_raw_fd(), file_name.as_ptr(), 0) } != 0 {
            return Err(fail("cannot remove completed revive journal"));
        }
        root.file
            .sync_all()
            .map_err(|error| fail(error.to_string()))?;
        Self::verify_pinned_agent_directory(&root)?;
        candidate.storage_directory = None;
        if reused {
            // Record the reuse for wrkslots' owner classification (#744, #777).
            let previous_pid = old
                .harness_identity
                .as_ref()
                .map(|identity| identity.pid)
                .or_else(|| old.custom_process_identity.as_ref().map(|i| i.pid));
            let current_pid = candidate
                .harness_identity
                .as_ref()
                .map(|identity| identity.pid)
                .or_else(|| candidate.custom_process_identity.as_ref().map(|i| i.pid));
            let current = (
                candidate.pane_id.clone(),
                candidate.terminal_id.clone(),
                current_pid,
            );
            super::record_reanchor(
                &mut candidate,
                (
                    Some(journal.proof.pane_id.clone()),
                    journal.proof.terminal_id.clone(),
                    previous_pid,
                ),
                current,
                "reuse",
            );
            self.save(&candidate)?;
        }
        let mut result = self.status_record(&candidate)?;
        result["revived"] = json!(true);
        result["previous_token"] = json!(journal.old_token);
        result["archive"] = json!(archive_path);
        result["pane_closed"] = json!(!keep_pane);
        result["tab_closed"] = json!(tab_closed);
        Ok(result)
    }

    /// Snapshot names and tokens, then recover independent eligible generations in order.
    pub fn revive_all(&self, dry_run: bool, startup_timeout: Duration) -> Result<Value> {
        if startup_timeout.is_zero() || startup_timeout > Duration::from_secs(300) {
            return Err(fail("startup timeout must be between 0 and 300 seconds"));
        }
        let mut generations = BTreeMap::new();
        if self.registry.exists() {
            agent::validate_private_directory(&self.registry, "agent registry", false)?;
            for name in self.registered_names()? {
                if let Some(record) = self.read_record_for_scan(&name)? {
                    generations.insert(name, record.token);
                }
            }
        }
        for journal in journals(&self.registry)? {
            generations.insert(journal.name, journal.old_token);
        }
        let mut agents = Vec::new();
        let mut revived = 0;
        let mut blocked = 0;
        for (name, expected) in generations {
            let outcome = (|| -> Result<Value> {
                let plan = self.revive(&name, true, Some(&expected), startup_timeout)?;
                if !dry_run && matches!(plan["action"].as_str(), Some("revive" | "recover")) {
                    self.revive(&name, false, Some(&expected), startup_timeout)
                } else {
                    Ok(plan)
                }
            })();
            match outcome {
                Ok(value) => {
                    if value["revived"] == true {
                        revived += 1;
                    }
                    if value["action"] == "blocked" {
                        blocked += 1;
                    }
                    agents.push(value);
                }
                Err(error) => {
                    blocked += 1;
                    let record = self.read_record(&name).ok();
                    agents.push(record.as_ref().map_or_else(|| json!({"name":name,"token":expected,"action":"blocked","reason":error.to_string()}), |record| self.revive_plan_document(record, "blocked", &error.to_string())));
                }
            }
        }
        Ok(json!({"dry_run":dry_run,"agents":agents,"revived":revived,"blocked":blocked}))
    }
}

fn queue_locks_at(directory: &Path) -> Result<Vec<File>> {
    let queue = directory.join("queue");
    if !queue.exists() {
        return Ok(Vec::new());
    }
    agent::validate_private_directory(&queue, "old revive queue", false)?;
    let mut held = Vec::new();
    for name in [".delivery.lock", ".binding.lock"] {
        let file = agent::open_private_lock(&queue.join(name), "old revive queue lock")?;
        file.lock_exclusive()
            .map_err(|error| fail(error.to_string()))?;
        held.push(file);
    }
    Ok(held)
}

fn staging_artifact_name(name: &str, artifact: &str) -> bool {
    let Some(suffix) = name.strip_prefix(&format!(".{artifact}-recovery-")) else {
        return false;
    };
    let parts: Vec<_> = suffix.split('-').collect();
    let decimal =
        |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    match parts.as_slice() {
        [pid, uuid] => decimal(pid) && journal_id_pattern(uuid),
        [pid, nanoseconds, attempt] => decimal(pid) && decimal(nanoseconds) && decimal(attempt),
        _ => false,
    }
}

/// Read entries through the pinned descriptor, including failures rather than guessing a census.
#[cfg(target_os = "linux")]
fn directory_names(pinned: &PinnedAgentDirectory) -> Result<BTreeSet<String>> {
    ManagedAgents::<HerdrClient>::verify_pinned_agent_directory(pinned)?;
    // Opening '.' gives an independent directory offset while preserving the pinned generation.
    let descriptor = unsafe {
        libc::openat(
            pinned.file.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        return Err(fail("cannot read pinned revive directory"));
    }
    let stream = unsafe { libc::fdopendir(descriptor) };
    if stream.is_null() {
        unsafe {
            libc::close(descriptor);
        }
        return Err(fail("cannot read pinned revive directory"));
    }
    let result = (|| -> Result<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        loop {
            // readdir uses the calling thread's errno to distinguish an error from EOF.
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                if io::Error::last_os_error().raw_os_error() != Some(0) {
                    return Err(fail("pinned revive directory census failed"));
                }
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }
                .to_str()
                .map_err(|_| fail("revive directory entry is not UTF-8"))?;
            if matches!(name, "." | "..") {
                continue;
            }
            if !names.insert(name.to_owned()) {
                return Err(fail("pinned revive directory census is ambiguous"));
            }
        }
        Ok(names)
    })();
    let closed = unsafe { libc::closedir(stream) };
    if closed != 0 {
        return Err(fail("cannot close pinned revive directory census"));
    }
    ManagedAgents::<HerdrClient>::verify_pinned_agent_directory(pinned)?;
    result
}

#[cfg(not(target_os = "linux"))]
fn directory_names(_pinned: &PinnedAgentDirectory) -> Result<BTreeSet<String>> {
    Err(fail(
        "revive setup recovery requires a supported pinned directory census",
    ))
}

fn publish_candidate(
    pinned: &PinnedAgentDirectory,
    destination: &Path,
    bytes: &[u8],
) -> Result<()> {
    let source_path = pinned
        .path
        .parent()
        .ok_or_else(|| fail("staged revive source has no parent"))?;
    let destination_path = destination
        .parent()
        .ok_or_else(|| fail("revive destination has no parent"))?;
    let source = ManagedAgents::<HerdrClient>::pinned_parent_directory(
        source_path,
        "revive operation directory",
    )?;
    let target =
        ManagedAgents::<HerdrClient>::pinned_parent_directory(destination_path, "agent registry")?;
    let destination_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| fail("invalid revive destination name"))?;
    if ManagedAgents::<HerdrClient>::child_directory_identity(&source.file, "new")?
        != Some((pinned.device, pinned.inode))
        || artifact_bytes(pinned, "agent.json")? != bytes
    {
        return Err(fail("staged revive changed before publication"));
    }
    rename_directory_noreplace_at(&source.file, "new", &target.file, destination_name)?;
    if ManagedAgents::<HerdrClient>::child_directory_identity(&source.file, "new")?.is_some()
        || ManagedAgents::<HerdrClient>::child_directory_identity(&target.file, destination_name)?
            != Some((pinned.device, pinned.inode))
    {
        return Err(fail(
            "revive publication result is uncertain; journal retained",
        ));
    }
    ManagedAgents::<HerdrClient>::verify_pinned_parent_directory(
        &source,
        "revive operation directory",
    )?;
    ManagedAgents::<HerdrClient>::verify_pinned_parent_directory(&target, "agent registry")?;
    source
        .file
        .sync_all()
        .map_err(|error| fail(error.to_string()))?;
    target
        .file
        .sync_all()
        .map_err(|error| fail(error.to_string()))?;
    Ok(())
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    pub(super) fn retire_revive_preflight(
        &self,
        record: &AgentRecord,
        destination: &Path,
        options: &RetireStaleOptions,
    ) -> Result<Option<(PathBuf, PathBuf)>> {
        let root = self.registry.join(".revives");
        let journal_path = root.join(format!("{}.json", record.token));
        let saved = destination.join(format!("{}.json", record.token));
        let present = journal_path.try_exists().map_err(|e| fail(e.to_string()))?;
        if !present && !saved.try_exists().map_err(|e| fail(e.to_string()))? {
            if options.expected_revive_sha256.is_some() || root.join(&record.token).exists() {
                return Err(fail("expected revive journal is missing; inspect retained artifacts before retirement"));
            }
            return Ok(None);
        }
        let expected = options.expected_revive_sha256.as_deref().ok_or_else(|| {
            fail("incomplete revive requires --expected-revive-sha256 after inspection")
        })?;
        let path = if present { &journal_path } else { &saved };
        let journal = parse_journal(path)?;
        if format!("{:x}", Sha256::digest(&journal.expected_bytes)) != expected {
            return Err(fail("revive journal SHA-256 changed"));
        }
        if journal.name != record.name
            || journal.old_token != record.token
            || !matches!(journal.phase.as_str(), "launching" | "ready")
        {
            return Err(fail(
                "revive journal is not this registration's incomplete transaction",
            ));
        }
        let operation = root.join(&record.token);
        let candidate = operation.join("new");
        if candidate.exists() {
            let (pin, candidate, bytes) = read_record_at(&candidate, &record.name)?;
            if candidate.token != journal.new_token
                || !same_journal_directory(pin.inode, journal.new_directory_inode)
                || journal
                    .ready_record_sha256
                    .as_ref()
                    .is_some_and(|expected| expected != &digest(&bytes))
            {
                return Err(fail("revive candidate generation or directory changed"));
            }
            if let Some(pane) = candidate.pane_id.as_deref() {
                let live = self.client.panes()?.iter().any(|row| row.pane_id == pane);
                // A pane holding only an idle shell has no harness to
                // preserve, so no session pin can (or must) name it; only a
                // candidate with a live harness needs the explicit pin.
                let dead_shell =
                    live && matches!(self.check_candidate(&candidate)?, CandidateCheck::Dead(_));
                if live && !dead_shell {
                    if options.replacement_pane.as_deref() != Some(pane) {
                        return Err(fail(format!(
                            "revive candidate pane {pane} is live; explicitly pin --replacement-pane {pane} and --replacement-session to preserve it"
                        )));
                    }
                    let session = options
                        .replacement_session
                        .as_ref()
                        .ok_or_else(|| fail("live candidate requires --replacement-session"))?;
                    agent::resolve_target(
                        self.client,
                        &Target {
                            pane_id: Some(pane.to_owned()),
                            session_agent: Some(candidate.harness.clone()),
                            session_value: Some(session.clone()),
                            expected_agent: Some(candidate.harness.clone()),
                            expected_workspace: self.project_workspace()?,
                            expected_cwd: Some(candidate.cwd.clone().into()),
                        },
                    )?;
                }
            }
        }
        Ok(Some((journal_path, operation)))
    }

    pub(super) fn retire_revive_archive(
        &self,
        record: &AgentRecord,
        destination: &Path,
        journal: &Path,
        operation: &Path,
        options: &RetireStaleOptions,
    ) -> Result<()> {
        self.retire_revive_preflight(record, destination, options)?;
        if journal.exists() {
            let saved = destination.join(format!("{}.json", record.token));
            if saved.exists() {
                return Err(fail(
                    "both current and archived revive journals exist; refusing overwrite",
                ));
            }
            let parent = pin_directory(
                journal.parent().expect("revive journal parent"),
                &record.name,
            )?;
            let archive = pin_directory(destination, &record.name)?;
            rename_directory_noreplace_at(
                &parent.file,
                journal
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| fail("invalid journal filename"))?,
                &archive.file,
                saved
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| fail("invalid archive filename"))?,
            )?;
            agent::sync_directory(journal.parent().expect("revive journal parent"))?;
            agent::sync_directory(destination)?;
        }
        if operation.exists() {
            let saved = destination.join("retired-revive");
            if saved.exists() {
                return Err(fail("both current and archived revive operation directories exist; refusing overwrite"));
            }
            // Pin and recheck the private directory; preserve every original artifact.
            let pinned = pin_directory(operation, &record.name)?;
            ManagedAgents::<A>::verify_pinned_agent_directory(&pinned)?;
            let parent = pin_directory(
                operation.parent().expect("revive operation parent"),
                &record.name,
            )?;
            let archive = pin_directory(destination, &record.name)?;
            rename_directory_noreplace_at(
                &parent.file,
                &record.token,
                &archive.file,
                "retired-revive",
            )?;
            let moved = fs::symlink_metadata(&saved).map_err(|e| fail(e.to_string()))?;
            if (moved.dev(), moved.ino()) != (pinned.device, pinned.inode) {
                return Err(fail(
                    "revive operation changed during archival; artifacts retained for inspection",
                ));
            }
            agent::sync_directory(operation.parent().expect("revive operation parent"))?;
        }
        agent::sync_directory(destination)
    }
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    /// Why a revive of `agent_name` could not resume its recorded conversation, when that is
    /// knowable: today, a Claude Code agent whose transcript is in none of the configuration
    /// directories that exist (`$CLAUDE_CONFIG_DIR`, `~/.claude`). `None` when the transcript
    /// is there, or when agentctl cannot tell (another harness, no configuration directory, or
    /// a launch profile that sets `CLAUDE_CONFIG_DIR`, which moves the transcripts).
    pub fn revive_unresumable_reason(&self, agent_name: &str) -> Result<Option<String>> {
        let record = self.read_record(agent_name)?;
        if record.harness != "claude"
            || fresh_revive(&record).is_some()
            || record
                .environment_names
                .iter()
                .any(|name| name == "CLAUDE_CONFIG_DIR")
        {
            return Ok(None);
        }
        let Ok(native) = recorded_conversation(&record) else {
            return Ok(None);
        };
        // Only a verdict of "missing" from a real configuration directory starts fresh: a wrong
        // guess would throw away a conversation that could have been resumed.
        Ok(
            (claude_transcript_verdict(&claude_config_roots(), &record.cwd, &native.value)
                == Some(false))
            .then(|| {
                format!(
                    "no Claude Code transcript for session {} remains, so it cannot be resumed",
                    native.value
                )
            }),
        )
    }

    /// Mark the dead record of `agent_name` so that its next revive starts a fresh
    /// conversation, recording `reason`; returns the new conversation's ID. A record already
    /// marked keeps its mark, so a retried revive starts the same conversation.
    pub fn mark_fresh_revive(&self, agent_name: &str, reason: &str) -> Result<String> {
        let _lock = self.lock(agent_name)?;
        if journals(&self.registry)?
            .iter()
            .any(|journal| journal.name == agent_name)
        {
            return Err(fail(format!(
                "an interrupted revive of {agent_name} is pending; finish it first with agentctl revive {agent_name}"
            )));
        }
        let mut record = self.read_record(agent_name)?;
        if let Some((session, _)) = fresh_revive(&record) {
            return Ok(session);
        }
        if crate::harness::adapter(&record.harness)
            .and_then(|adapter| adapter.fresh_arguments(Vec::new(), "probe"))
            .is_none()
        {
            return Err(fail(format!(
                "{} cannot be started on a conversation ID agentctl chooses, so {agent_name} cannot be revived fresh",
                record.harness
            )));
        }
        if self.revive_liveness(&record) == ProcessLiveness::Alive {
            return Err(fail(format!(
                "{agent_name}'s recorded harness is alive; nothing to revive"
            )));
        }
        let session = new_conversation_id()?;
        record.extra.insert(
            FRESH_REVIVE.to_owned(),
            json!({"schema": FRESH_REVIVE_SCHEMA, "session": session, "reason": reason}),
        );
        self.save(&record)?;
        Ok(session)
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    fn rec(cwd: &str) -> AgentRecord {
        AgentRecord {
            storage_directory: None,
            nested: None,
            adapter: "herdr".into(),
            mode: "interactive".into(),
            backend: "herdr".into(),
            paused: false,
            runtime_home: None,
            pane_reported_by_agentctl: false,
            custom_process_identity: None,
            foreign_shell_identity: None,
            terminal_id: None,
            harness_identity: None,
            name_history: vec![],
            anchor_rule: None,
            former_names: vec![],
            native_session: None,
            profile: None,
            slot: None,
            slot_project: None,
            slot_isolation: None,
            reasoning_effort: None,
            environment_names: vec![],
            agentcloud: None,
            launch_spec: None,
            extra: BTreeMap::new(),
            name: "lane".into(),
            token: "t".into(),
            harness: "claude".into(),
            cwd: cwd.into(),
            created_at: 0.0,
            schema: 1,
            lifecycle: "stopped".into(),
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            session_agent: None,
            session_value: None,
            model: None,
            resume: None,
            arguments: vec![],
            startup_warning: None,
            effective_reasoning_effort: None,
            error: None,
            goal: None,
            goal_delivery: None,
            goal_session_id: None,
            goal_command: None,
            goal_messages: BTreeMap::new(),
            goal_message_id: None,
        }
    }
    #[test]
    fn discovers_newest_confirmed_claude_session() {
        let root = std::env::temp_dir().join(format!("disc-{}", std::process::id()));
        let dir = root.join("projects").join("-tmp-proj");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
            "{\"cwd\":\"/tmp/proj\"}\n",
        )
        .unwrap();
        fs::write(
            dir.join("22222222-2222-2222-2222-222222222222.jsonl"),
            "{\"cwd\":\"/other\"}\n",
        )
        .unwrap();
        let got = discover_claude_session(std::slice::from_ref(&root), &rec("/tmp/proj"));
        assert_eq!(
            got.map(|s| s.value).as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        let _ = fs::remove_dir_all(&root);
    }
    #[test]
    fn muse_fresh_selector_adds_no_resume() {
        let args = fresh_selector("muse", vec!["--model".into(), "m".into()], "sess-1").unwrap();
        assert_eq!(args, vec!["--model".to_string(), "m".to_string()]);
    }
    #[test]
    fn newest_archive_picks_latest_for_name() {
        let root = std::env::temp_dir().join(format!("arch-{}", std::process::id()));
        fs::create_dir_all(root.join("lane-tok1")).unwrap();
        fs::create_dir_all(root.join("lane-tok2")).unwrap();
        fs::create_dir_all(root.join("other-tok")).unwrap();
        let got = newest_archive(&root, "lane");
        assert!(got.is_some_and(|p| p
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("lane-tok")));
        assert!(newest_archive(&root, "missing").is_none());
        let _ = fs::remove_dir_all(&root);
    }
    #[test]
    fn refuses_without_content_confirmation() {
        let root = std::env::temp_dir().join(format!("disc2-{}", std::process::id()));
        let dir = root.join("projects").join("-tmp-proj");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("11111111-1111-1111-1111-111111111111.jsonl"),
            "{\"cwd\":\"/other\"}\n",
        )
        .unwrap();
        assert!(discover_claude_session(std::slice::from_ref(&root), &rec("/tmp/proj")).is_none());
        let _ = fs::remove_dir_all(&root);
    }
}
