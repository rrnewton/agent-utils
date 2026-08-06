//! Idempotent resolution of the Herdr server/workspace/tab/pane chain.
//!
//! The on-disk cache is only an optimization.  Every cached ID is checked against live Herdr
//! state, including the workspace and tab labels, before it can select a pane.  Cache updates use
//! an inter-process advisory lock and a unique temporary-file rename so concurrent callers cannot
//! tear the document or silently discard each other's entries.

use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::client::HerdrApi;
use crate::config::{render_tab_name, Config};
use crate::error::{HerdrRunError, Result};
use crate::state::{open_lock_file, session_lock_path};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A fully resolved destination pane and the session objects created to obtain it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    /// Herdr workspace identifier.
    pub workspace_id: String,
    /// Herdr tab identifier.
    pub tab_id: String,
    /// Herdr pane identifier.
    pub pane_id: String,
    /// Expected and live-validated workspace label.
    pub workspace_label: String,
    /// Expected and live-validated tab label.
    pub tab_label: String,
    /// Session levels created by this resolution, in creation order.
    pub created: Vec<String>,
    /// Whether the result came from a revalidated cache entry.
    pub from_cache: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CacheRecord {
    workspace_id: String,
    tab_id: String,
    pane_id: String,
    #[serde(default)]
    workspace_label: Option<String>,
    #[serde(default)]
    tab_label: Option<String>,
}

/// Render the configured restricted tab-label template for `agent`.
///
/// Only literal text, doubled opening or closing braces, and the exact `agent` and `project`
/// placeholders are accepted. Attribute access, conversions, format specifications, positional
/// fields, and unmatched braces are configuration errors.
pub fn tab_label_for(config: &Config, agent: &str) -> Result<String> {
    let project = Path::new(&config.project_root)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project");
    render_tab_name(&config.tab_name, agent, project)
}

/// Return the session-cache path for `config`.
#[must_use]
pub fn cache_path(config: &Config) -> PathBuf {
    Path::new(&config.project_root)
        .join(&config.spool_dir)
        .join("session-cache.json")
}

/// Ensure and resolve the destination pane for `agent`.
///
/// When `use_cache` is true, a cache hit is used only after all IDs and labels agree with the live
/// session.  Cache read or write failures never prevent label-based resolution.
pub fn resolve_target<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    agent: &str,
    use_cache: bool,
) -> Result<Target> {
    let lock_path = session_lock_path()?;
    let lock = open_lock_file(&lock_path)?;
    FileExt::lock_exclusive(&lock).map_err(|error| {
        HerdrRunError::unavailable(format!(
            "cannot lock Herdr session resolution at {}: {error}",
            lock_path.display()
        ))
    })?;
    resolve_target_locked(client, config, agent, use_cache)
}

fn resolve_target_locked<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    agent: &str,
    use_cache: bool,
) -> Result<Target> {
    let tab_label = tab_label_for(config, agent)?;
    let cwd = resolved_cwd(config)?;
    let cwd = cwd.to_str().ok_or_else(|| {
        HerdrRunError::config(format!(
            "working directory is not valid UTF-8: {}",
            cwd.display()
        ))
    })?;
    let key = format!("{}\0{tab_label}", config.workspace);
    let path = cache_path(config);
    let mut created = Vec::new();

    if client.ensure_server()? {
        created.push("server".to_owned());
    }

    if use_cache && created.is_empty() {
        if let Some(cached) = load_cache(&path, &key) {
            if cache_still_valid(client, &cached, &config.workspace, &tab_label)? {
                return Ok(Target {
                    workspace_id: cached.workspace_id,
                    tab_id: cached.tab_id,
                    pane_id: cached.pane_id,
                    workspace_label: config.workspace.clone(),
                    tab_label,
                    created,
                    from_cache: true,
                });
            }
        }
    }

    let (workspace_id, tab_id) = match client.workspace_id_for_label(&config.workspace)? {
        Some(workspace_id) => {
            let tab_id = match client.tab_id_for_label(&workspace_id, &tab_label)? {
                Some(tab_id) => tab_id,
                None => {
                    let tab_id = client.create_tab(&workspace_id, &tab_label, cwd)?;
                    created.push("tab".to_owned());
                    tab_id
                }
            };
            (workspace_id, tab_id)
        }
        None => {
            // A new workspace already has one root tab. Rename it instead of leaving a redundant
            // default tab and adding another one.
            let (workspace_id, root_tab_id, _root_pane_id) =
                client.create_workspace(&config.workspace, cwd)?;
            client.rename_tab(&root_tab_id, &tab_label)?;
            created.extend(["workspace".to_owned(), "tab".to_owned()]);
            (workspace_id, root_tab_id)
        }
    };

    let pane_id = pane_of_tab(client, &workspace_id, &tab_id)?;
    let target = Target {
        workspace_id,
        tab_id,
        pane_id,
        workspace_label: config.workspace.clone(),
        tab_label,
        created,
        from_cache: false,
    };
    // The cache is deliberately non-authoritative. A read-only or damaged spool must not turn a
    // usable live Herdr session into a failed command.
    let _ = store_cache(&path, &key, &target);
    Ok(target)
}

fn resolved_cwd(config: &Config) -> Result<PathBuf> {
    let root = Path::new(&config.project_root);
    if !root.is_absolute() {
        return Err(HerdrRunError::config(format!(
            "project_root must be absolute: {}",
            root.display()
        )));
    }
    let cwd = config.cwd.as_deref().map(Path::new).unwrap_or(root);
    let joined = if cwd.is_absolute() {
        cwd.to_path_buf()
    } else {
        root.join(cwd)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.parent().is_some() {
                    normalized.pop();
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

fn pane_of_tab<A: HerdrApi + ?Sized>(
    client: &A,
    workspace_id: &str,
    tab_id: &str,
) -> Result<String> {
    let panes = client
        .panes(Some(workspace_id))?
        .into_iter()
        .filter(|pane| pane.tab_id == tab_id)
        .collect::<Vec<_>>();
    match panes.as_slice() {
        [] => Err(HerdrRunError::unavailable(format!(
            "tab {tab_id} has no pane"
        ))),
        [pane] => Ok(pane.pane_id.clone()),
        _ => {
            let ids = panes
                .iter()
                .map(|pane| pane.pane_id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(HerdrRunError::unavailable(format!(
                "tab {tab_id} has {} panes ({ids}); herdr-run needs an unsplit tab. Close the extra panes or point tab_name at a different tab.",
                panes.len()
            )))
        }
    }
}

fn cache_still_valid<A: HerdrApi + ?Sized>(
    client: &A,
    cached: &CacheRecord,
    workspace_label: &str,
    tab_label: &str,
) -> Result<bool> {
    if cached
        .workspace_label
        .as_deref()
        .is_some_and(|label| label != workspace_label)
        || cached
            .tab_label
            .as_deref()
            .is_some_and(|label| label != tab_label)
        || !client.pane_exists(&cached.pane_id)
    {
        return Ok(false);
    }
    // Resolve by the configured label, not merely by cached ID. Besides detecting ID reuse, this
    // exercises the client's duplicate-label refusal instead of letting a cache hide ambiguity.
    if client.workspace_id_for_label(workspace_label)?.as_deref()
        != Some(cached.workspace_id.as_str())
    {
        return Ok(false);
    }
    if client
        .tab_id_for_label(&cached.workspace_id, tab_label)?
        .as_deref()
        != Some(cached.tab_id.as_str())
    {
        return Ok(false);
    }
    Ok(client
        .panes(Some(&cached.workspace_id))?
        .iter()
        .any(|pane| {
            pane.pane_id == cached.pane_id
                && pane.tab_id == cached.tab_id
                && pane.workspace_id == cached.workspace_id
        }))
}

fn load_cache(path: &Path, key: &str) -> Option<CacheRecord> {
    let contents = fs::read_to_string(path).ok()?;
    let document = serde_json::from_str::<serde_json::Value>(&contents).ok()?;
    let value = document.as_object()?.get(key)?.clone();
    serde_json::from_value(value).ok()
}

fn store_cache(path: &Path, key: &str, target: &Target) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache has no parent"))?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(lock_path)?;
    lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    FileExt::lock_exclusive(&lock)?;

    let mut entries = fs::read_to_string(path)
        .ok()
        .and_then(|contents| {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&contents).ok()
        })
        .unwrap_or_default();
    let record = CacheRecord {
        workspace_id: target.workspace_id.clone(),
        tab_id: target.tab_id.clone(),
        pane_id: target.pane_id.clone(),
        workspace_label: Some(target.workspace_label.clone()),
        tab_label: Some(target.tab_label.clone()),
    };
    entries.insert(
        key.to_owned(),
        serde_json::to_value(record).map_err(io::Error::other)?,
    );

    let temporary = unique_temporary(parent);
    let write_result =
        write_cache_temporary(&temporary, &entries).and_then(|()| fs::rename(&temporary, path));
    if write_result.is_ok() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn unique_temporary(parent: &Path) -> PathBuf {
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".session-cache.{}.{}.tmp",
        std::process::id(),
        sequence
    ))
}

fn write_cache_temporary(
    path: &Path,
    entries: &serde_json::Map<String, serde_json::Value>,
) -> io::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, entries).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    writer.get_ref().sync_all()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc, Barrier, Condvar, Mutex};
    use std::thread;

    use crate::client::{Pane, ProcessInfo};

    use super::*;

    #[derive(Clone)]
    struct FakeHerdr {
        state: Arc<Mutex<FakeState>>,
        create_gate: Option<Arc<CreateGate>>,
    }

    struct CreateGate {
        calls: AtomicUsize,
        entered: (Mutex<bool>, Condvar),
        released: (Mutex<bool>, Condvar),
    }

    impl CreateGate {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                entered: (Mutex::new(false), Condvar::new()),
                released: (Mutex::new(false), Condvar::new()),
            }
        }

        fn block_first_create(&self) {
            let ordinal = self.calls.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            if ordinal != 1 {
                return;
            }
            let (entered, entered_cv) = &self.entered;
            *entered.lock().expect("entered") = true;
            entered_cv.notify_all();
            let (released, released_cv) = &self.released;
            let mut release = released.lock().expect("released");
            while !*release {
                release = released_cv.wait(release).expect("release wait");
            }
        }

        fn wait_until_entered(&self) {
            let (entered, entered_cv) = &self.entered;
            let mut value = entered.lock().expect("entered");
            while !*value {
                value = entered_cv.wait(value).expect("entered wait");
            }
        }

        fn release(&self) {
            let (released, released_cv) = &self.released;
            *released.lock().expect("released") = true;
            released_cv.notify_all();
        }
    }

    struct FakeState {
        start_server: bool,
        workspace_id: Option<String>,
        workspace_label: Option<String>,
        tab_id: Option<String>,
        tab_label: Option<String>,
        panes: Vec<Pane>,
        calls: Vec<String>,
    }

    impl Default for FakeHerdr {
        fn default() -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeState {
                    start_server: false,
                    workspace_id: None,
                    workspace_label: None,
                    tab_id: None,
                    tab_label: None,
                    panes: Vec::new(),
                    calls: Vec::new(),
                })),
                create_gate: None,
            }
        }
    }

    impl FakeHerdr {
        fn calls(&self) -> Vec<String> {
            self.state.lock().expect("state").calls.clone()
        }
    }

    impl HerdrApi for FakeHerdr {
        fn ensure_server(&self) -> Result<bool> {
            let mut state = self.state.lock().expect("state");
            state.calls.push("ensure".to_owned());
            Ok(std::mem::take(&mut state.start_server))
        }

        fn workspace_id_for_label(&self, label: &str) -> Result<Option<String>> {
            let mut state = self.state.lock().expect("state");
            state.calls.push(format!("workspace:{label}"));
            Ok((state.workspace_label.as_deref() == Some(label))
                .then(|| state.workspace_id.clone())
                .flatten())
        }

        fn workspace_label_for_id(&self, workspace_id: &str) -> Result<Option<String>> {
            let state = self.state.lock().expect("state");
            Ok((state.workspace_id.as_deref() == Some(workspace_id))
                .then(|| state.workspace_label.clone())
                .flatten())
        }

        fn create_workspace(&self, label: &str, _cwd: &str) -> Result<(String, String, String)> {
            if let Some(gate) = &self.create_gate {
                gate.block_first_create();
            }
            let mut state = self.state.lock().expect("state");
            state.calls.push("create-workspace".to_owned());
            state.workspace_id = Some("w1".to_owned());
            state.workspace_label = Some(label.to_owned());
            state.tab_id = Some("t1".to_owned());
            state.panes = vec![Pane {
                pane_id: "p1".to_owned(),
                tab_id: "t1".to_owned(),
                workspace_id: "w1".to_owned(),
            }];
            Ok(("w1".to_owned(), "t1".to_owned(), "p1".to_owned()))
        }

        fn tab_id_for_label(&self, workspace_id: &str, label: &str) -> Result<Option<String>> {
            let state = self.state.lock().expect("state");
            if state.workspace_id.as_deref() != Some(workspace_id)
                || state.tab_label.as_deref() != Some(label)
            {
                return Ok(None);
            }
            Ok(state.tab_id.clone())
        }

        fn create_tab(&self, workspace_id: &str, label: &str, _cwd: &str) -> Result<String> {
            let mut state = self.state.lock().expect("state");
            state.calls.push("create-tab".to_owned());
            state.tab_id = Some("t2".to_owned());
            state.tab_label = Some(label.to_owned());
            state.panes = vec![Pane {
                pane_id: "p2".to_owned(),
                tab_id: "t2".to_owned(),
                workspace_id: workspace_id.to_owned(),
            }];
            Ok("t2".to_owned())
        }

        fn rename_tab(&self, tab_id: &str, label: &str) -> Result<()> {
            let mut state = self.state.lock().expect("state");
            state.calls.push("rename-tab".to_owned());
            state.tab_id = Some(tab_id.to_owned());
            state.tab_label = Some(label.to_owned());
            Ok(())
        }

        fn panes(&self, workspace_id: Option<&str>) -> Result<Vec<Pane>> {
            let state = self.state.lock().expect("state");
            Ok(state
                .panes
                .iter()
                .filter(|pane| workspace_id.is_none_or(|wanted| pane.workspace_id == wanted))
                .cloned()
                .collect())
        }

        fn pane_exists(&self, pane_id: &str) -> bool {
            self.state
                .lock()
                .expect("state")
                .panes
                .iter()
                .any(|pane| pane.pane_id == pane_id)
        }

        fn process_info(&self, _pane_id: &str) -> Result<ProcessInfo> {
            unreachable!()
        }

        fn read(&self, _pane_id: &str, _source: &str, _lines: Option<usize>) -> Result<String> {
            unreachable!()
        }

        fn run(&self, _pane_id: &str, _command: &str) -> Result<()> {
            unreachable!()
        }

        fn send_keys(&self, _pane_id: &str, _keys: &str) -> Result<()> {
            unreachable!()
        }
    }

    fn temporary_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "herdr-run-session-{name}-{}-{}",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temporary root");
        path
    }

    fn config(root: &Path) -> Config {
        Config {
            project_root: root.to_string_lossy().into_owned(),
            ..Config::default()
        }
    }

    #[test]
    fn restricted_tab_renderer_accepts_only_exact_fields_and_escaped_braces() {
        let root = temporary_root("render").join("project-x");
        fs::create_dir_all(&root).unwrap();
        let mut config = config(&root);
        config.tab_name = "{{{project}}}:{agent}".to_owned();
        assert_eq!(
            tab_label_for(&config, "worker").unwrap(),
            "{project-x}:worker"
        );
        for invalid in [
            "{}",
            "{agent.name}",
            "{agent!r}",
            "{agent:>3}",
            "{agent:",
            "x}",
        ] {
            config.tab_name = invalid.to_owned();
            assert!(tab_label_for(&config, "worker").is_err(), "{invalid}");
        }
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    #[test]
    fn first_resolution_reuses_root_tab_and_second_uses_validated_cache() {
        let root = temporary_root("cache-hit");
        let config = config(&root);
        let fake = FakeHerdr::default();
        let first = resolve_target(&fake, &config, "agent", true).unwrap();
        assert_eq!(first.created, ["workspace", "tab"]);
        assert!(!first.from_cache);
        let second = resolve_target(&fake, &config, "agent", true).unwrap();
        assert!(second.from_cache);
        assert_eq!(first.pane_id, second.pane_id);
        assert_eq!(
            fake.calls()
                .iter()
                .filter(|call| call.as_str() == "create-workspace")
                .count(),
            1
        );
        let path = cache_path(&config);
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(format!("{}.lock", path.display()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_first_resolution_across_projects_creates_session_once() {
        let root = temporary_root("concurrent-resolution");
        let project_one = root.join("project-one");
        let project_two = root.join("project-two");
        fs::create_dir_all(&project_one).unwrap();
        fs::create_dir_all(&project_two).unwrap();
        let first_config = config(&project_one);
        let second_config = config(&project_two);
        let gate = Arc::new(CreateGate::new());
        let fake = FakeHerdr {
            create_gate: Some(gate.clone()),
            ..FakeHerdr::default()
        };

        let first_fake = fake.clone();
        let first =
            thread::spawn(move || resolve_target(&first_fake, &first_config, "agent", true));
        gate.wait_until_entered();

        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let second_fake = fake.clone();
        let second = thread::spawn(move || {
            started_tx.send(()).unwrap();
            resolve_target(&second_fake, &second_config, "agent", true)
        });
        started_rx.recv().unwrap();
        thread::sleep(std::time::Duration::from_millis(100));
        let calls_while_first_blocked = gate.calls.load(AtomicOrdering::SeqCst);
        gate.release();

        let first_target = first.join().unwrap().unwrap();
        let second_target = second.join().unwrap().unwrap();
        assert_eq!(calls_while_first_blocked, 1);
        assert_eq!(gate.calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(first_target.pane_id, second_target.pane_id);
        assert_eq!(
            fake.calls()
                .iter()
                .filter(|call| call.as_str() == "create-workspace")
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_rejects_reused_workspace_id_with_wrong_label() {
        let root = temporary_root("stale-label");
        let config = config(&root);
        let fake = FakeHerdr::default();
        resolve_target(&fake, &config, "agent", true).unwrap();
        {
            let mut state = fake.state.lock().unwrap();
            state.workspace_label = Some("someone-else".to_owned());
        }
        let second = resolve_target(&fake, &config, "agent", true).unwrap();
        assert!(!second.from_cache);
        assert_eq!(second.created, ["workspace", "tab"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn split_tab_is_refused_instead_of_selecting_an_arbitrary_pane() {
        let root = temporary_root("split");
        let config = config(&root);
        let fake = FakeHerdr::default();
        resolve_target(&fake, &config, "agent", true).unwrap();
        {
            let mut state = fake.state.lock().unwrap();
            state.panes.push(Pane {
                pane_id: "p-extra".to_owned(),
                tab_id: "t1".to_owned(),
                workspace_id: "w1".to_owned(),
            });
        }
        let error = resolve_target(&fake, &config, "agent", false).unwrap_err();
        assert!(error.to_string().contains("needs an unsplit tab"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_cache_writers_preserve_both_entries() {
        let root = temporary_root("writers");
        let config = config(&root);
        let path = cache_path(&config);
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for index in 0..2 {
            let barrier = barrier.clone();
            let path = path.clone();
            workers.push(thread::spawn(move || {
                let target = Target {
                    workspace_id: format!("w{index}"),
                    tab_id: format!("t{index}"),
                    pane_id: format!("p{index}"),
                    workspace_label: "workspace".to_owned(),
                    tab_label: format!("agent-{index}"),
                    created: Vec::new(),
                    from_cache: false,
                };
                barrier.wait();
                store_cache(&path, &format!("key-{index}"), &target).unwrap();
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        let document: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(document.len(), 2);
        assert!(document.contains_key("key-0"));
        assert!(document.contains_key("key-1"));
        fs::remove_dir_all(root).unwrap();
    }
}
