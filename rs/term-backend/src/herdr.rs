//! The Herdr backend: the Herdr CLI driven with the argument shapes
//! existing Herdr clients already use. Argument shapes and JSON field
//! validation match those clients where their tests pin them; this
//! backend deliberately does not reproduce their per-call timeouts,
//! cancellation, or process-identity proofs — consumers that need those
//! keep driving their own client behind the same traits.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::runner::{detail, expect_ok, system_runner, CommandRunner, SharedRunner};
use crate::{
    strings, AgentStatus, Capabilities, CreatedPane, Multiplexer, PaneId, PaneInfo, PaneTerminal,
    Result, ScreenRead, TabId, TabInfo, TermError, WorkspaceId, WorkspaceInfo,
};

/// A [`crate::TermBackend`] that drives the Herdr CLI.
pub struct HerdrBackend {
    executable: PathBuf,
    runner: SharedRunner,
}

impl HerdrBackend {
    /// Drive `executable` (a name on `PATH` or an explicit path).
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            runner: system_runner(),
        }
    }

    /// Drive `executable` through a caller-supplied command runner (tests).
    pub fn with_runner(executable: PathBuf, runner: Arc<dyn CommandRunner>) -> Self {
        Self { executable, runner }
    }

    fn call(&self, args: &[String], purpose: &str) -> Result<Map<String, Value>> {
        let output = self.runner.run(&self.executable, args, None)?;
        if output.status != 0 {
            return Err(TermError::failed(format!("{purpose}: {}", detail(&output))));
        }
        let document = serde_json::from_str::<Value>(&output.stdout).map_err(|_| {
            let preview: String = output.stdout.trim().chars().take(200).collect();
            TermError::failed(format!(
                "{purpose}: herdr returned non-JSON output: {preview:?}"
            ))
        })?;
        let envelope = document.as_object().ok_or_else(|| {
            TermError::failed(format!("{purpose}: Herdr response is not an object"))
        })?;
        envelope
            .get("result")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| {
                TermError::failed(format!("{purpose}: Herdr response has no result object"))
            })
    }

    fn call_ok(&self, args: &[String], purpose: &str) -> Result<()> {
        expect_ok(&self.runner.run(&self.executable, args, None)?, purpose)
    }

    fn workspace_entries(&self) -> Result<Vec<Map<String, Value>>> {
        let result = self.call(&strings(&["workspace", "list"]), "workspace list")?;
        object_entries(result.get("workspaces"), "workspace list entry")
    }
}

impl Multiplexer for HerdrBackend {
    fn backend_name(&self) -> &'static str {
        "herdr"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            native_agent_status: true,
            native_prompt: true,
        }
    }

    fn list_workspaces(&self) -> Result<Vec<WorkspaceInfo>> {
        self.workspace_entries()?
            .iter()
            .map(|entry| {
                Ok(WorkspaceInfo {
                    id: required_string(entry, "workspace_id", "workspace list entry")?.into(),
                    label: required_string(entry, "label", "workspace list entry")?,
                })
            })
            .collect()
    }

    fn create_workspace(
        &self,
        label: &str,
        cwd: &std::path::Path,
        environment: &[String],
    ) -> Result<CreatedPane> {
        let mut args = strings(&["workspace", "create", "--label", label, "--cwd"]);
        args.push(cwd.display().to_string());
        append_environment(&mut args, environment);
        let result = self.call(&args, &format!("workspace create {label:?}"))?;
        Ok(CreatedPane {
            workspace_id: required_string(
                required_object(&result, "workspace", "workspace create")?,
                "workspace_id",
                "workspace create",
            )?
            .into(),
            tab_id: required_string(
                required_object(&result, "tab", "workspace create")?,
                "tab_id",
                "workspace create",
            )?
            .into(),
            pane_id: required_string(
                required_object(&result, "root_pane", "workspace create")?,
                "pane_id",
                "workspace create",
            )?
            .into(),
        })
    }

    fn focus_workspace(&self, workspace: &WorkspaceId) -> Result<()> {
        self.call_ok(
            &strings(&["workspace", "focus", workspace.as_str()]),
            "workspace focus",
        )
    }

    fn rename_workspace(&self, workspace: &WorkspaceId, label: &str) -> Result<()> {
        self.call(
            &strings(&["workspace", "rename", workspace.as_str(), label]),
            &format!("workspace rename {workspace}"),
        )?;
        Ok(())
    }

    fn close_workspace(&self, workspace: &WorkspaceId) -> Result<()> {
        self.call_ok(
            &strings(&["workspace", "close", workspace.as_str()]),
            "workspace close",
        )
    }

    fn list_tabs(&self, workspace: &WorkspaceId) -> Result<Vec<TabInfo>> {
        let result = self.call(
            &strings(&["tab", "list", "--workspace", workspace.as_str()]),
            "tab list",
        )?;
        object_entries(result.get("tabs"), "tab list entry")?
            .iter()
            .map(|entry| {
                Ok(TabInfo {
                    id: required_string(entry, "tab_id", "tab list entry")?.into(),
                    workspace_id: workspace.clone(),
                    label: required_string(entry, "label", "tab list entry")?,
                })
            })
            .collect()
    }

    fn create_tab(
        &self,
        workspace: &WorkspaceId,
        label: &str,
        cwd: &std::path::Path,
        environment: &[String],
    ) -> Result<CreatedPane> {
        let mut args = strings(&[
            "tab",
            "create",
            "--workspace",
            workspace.as_str(),
            "--label",
            label,
            "--cwd",
        ]);
        args.push(cwd.display().to_string());
        append_environment(&mut args, environment);
        let result = self.call(&args, &format!("tab create {label:?}"))?;
        let tab_id: TabId = required_string(
            required_object(&result, "tab", "tab create")?,
            "tab_id",
            "tab create",
        )?
        .into();
        let pane = required_object(&result, "root_pane", "tab create")?;
        if required_string(pane, "tab_id", "created pane")? != tab_id.as_str()
            || required_string(pane, "workspace_id", "created pane")? != workspace.as_str()
        {
            return Err(TermError::failed(
                "created pane does not belong to the allocated tab/workspace",
            ));
        }
        Ok(CreatedPane {
            workspace_id: workspace.clone(),
            tab_id,
            pane_id: required_string(pane, "pane_id", "tab create")?.into(),
        })
    }

    fn focus_tab(&self, tab: &TabId) -> Result<()> {
        self.call_ok(&strings(&["tab", "focus", tab.as_str()]), "tab focus")
    }

    fn rename_tab(&self, tab: &TabId, label: &str) -> Result<()> {
        self.call(
            &strings(&["tab", "rename", tab.as_str(), label]),
            &format!("tab rename {tab}"),
        )?;
        Ok(())
    }

    fn close_tab(&self, tab: &TabId) -> Result<()> {
        self.call_ok(
            &strings(&["tab", "close", tab.as_str()]),
            &format!("tab close {tab}"),
        )
    }

    fn list_panes(&self, workspace: Option<&WorkspaceId>) -> Result<Vec<PaneInfo>> {
        let mut args = strings(&["pane", "list"]);
        if let Some(workspace) = workspace {
            args.extend(strings(&["--workspace", workspace.as_str()]));
        }
        let result = self.call(&args, "pane list")?;
        let panes: Vec<PaneInfo> = object_entries(result.get("panes"), "pane list entry")?
            .iter()
            .map(|entry| {
                Ok(PaneInfo {
                    id: required_string(entry, "pane_id", "pane list entry")?.into(),
                    tab_id: Some(required_string(entry, "tab_id", "pane list entry")?.into()),
                    workspace_id: Some(
                        required_string(entry, "workspace_id", "pane list entry")?.into(),
                    ),
                    cwd: optional_string(entry, "cwd")?.map(PathBuf::from),
                    title: None,
                    agent: optional_string(entry, "agent")?,
                    agent_status: optional_string(entry, "agent_status")?
                        .map(|raw| AgentStatus::parse(&raw)),
                    pid: None,
                    terminal_id: optional_string(entry, "terminal_id")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if let Some(expected) = workspace {
            if let Some(pane) = panes
                .iter()
                .find(|pane| pane.workspace_id.as_ref() != Some(expected))
            {
                return Err(TermError::failed(format!(
                    "pane list: returned pane {:?} from a different workspace, expected {expected:?}",
                    pane.id
                )));
            }
        }
        Ok(panes)
    }

    fn focus_pane(&self, pane: &PaneId) -> Result<()> {
        self.call_ok(&strings(&["agent", "focus", pane.as_str()]), "agent focus")
    }

    fn close_pane(&self, pane: &PaneId) -> Result<()> {
        self.call_ok(&strings(&["pane", "close", pane.as_str()]), "pane close")
    }
}

impl PaneTerminal for HerdrBackend {
    fn pane_info(&self, pane: &PaneId) -> Result<PaneInfo> {
        let result = self.call(
            &strings(&["pane", "get", pane.as_str()]),
            &format!("pane get {pane}"),
        )?;
        let entry = required_object(&result, "pane", "pane get")?;
        let returned = required_string(entry, "pane_id", "pane get")?;
        if returned != pane.as_str() {
            return Err(TermError::failed(format!(
                "pane get: returned pane {returned:?}, expected {pane:?}"
            )));
        }
        Ok(PaneInfo {
            id: pane.clone(),
            tab_id: optional_string(entry, "tab_id")?.map(TabId::from),
            workspace_id: Some(required_string(entry, "workspace_id", "pane get")?.into()),
            cwd: Some(PathBuf::from(required_string(entry, "cwd", "pane get")?)),
            title: None,
            agent: optional_string(entry, "agent")?,
            agent_status: Some(AgentStatus::parse(
                optional_string(entry, "agent_status")?
                    .as_deref()
                    .unwrap_or("unknown"),
            )),
            pid: None,
            terminal_id: optional_string(entry, "terminal_id")?,
        })
    }

    fn read_screen(&self, pane: &PaneId, read: ScreenRead) -> Result<String> {
        let (source, lines) = match read.scrollback_lines {
            Some(0) => ("visible", None),
            Some(lines) => ("recent", Some(lines)),
            None => ("recent", None),
        };
        let mut args = strings(&["pane", "read", pane.as_str(), "--source", source]);
        if let Some(lines) = lines {
            args.extend(["--lines".to_owned(), lines.to_string()]);
        }
        if read.ansi {
            args.extend(strings(&["--format", "ansi"]));
        }
        let output = self.runner.run(&self.executable, &args, None)?;
        expect_ok(&output, &format!("pane read {pane}"))?;
        Ok(output.stdout)
    }

    fn paste_text(&self, pane: &PaneId, text: &str) -> Result<()> {
        self.call_ok(
            &strings(&["pane", "send-text", pane.as_str(), text]),
            &format!("pane send-text {pane}"),
        )
    }

    fn send_keys(&self, pane: &PaneId, keys: &str) -> Result<()> {
        self.call_ok(
            &strings(&["pane", "send-keys", pane.as_str(), keys]),
            &format!("pane send-keys {pane}"),
        )
    }

    fn prompt(&self, pane: &PaneId, text: &str) -> Result<()> {
        self.call_ok(
            &strings(&["agent", "prompt", pane.as_str(), text]),
            &format!("agent prompt {pane}"),
        )
    }

    fn wait_agent_status(
        &self,
        pane: &PaneId,
        status: &AgentStatus,
        timeout_ms: u64,
    ) -> Result<()> {
        let purpose = format!("wait for pane {pane} status {}", status.as_str());
        let args = vec![
            "agent".to_owned(),
            "wait".to_owned(),
            pane.to_string(),
            "--until".to_owned(),
            status.as_str().to_owned(),
            "--timeout".to_owned(),
            timeout_ms.to_string(),
        ];
        let output = self.runner.run(&self.executable, &args, None)?;
        expect_ok(&output, &purpose)?;
        let document = serde_json::from_str::<Value>(&output.stdout).map_err(|error| {
            TermError::failed(format!("{purpose}: invalid Herdr event response: {error}"))
        })?;
        let envelope = document.as_object().ok_or_else(|| {
            TermError::failed(format!("{purpose}: event response is not an object"))
        })?;
        let result = required_object(envelope, "result", &purpose)?;
        let data = required_object(result, "agent", &purpose)?;
        if required_string(data, "pane_id", &purpose)? != pane.as_str()
            || required_string(data, "agent_status", &purpose)? != status.as_str()
        {
            return Err(TermError::failed(format!(
                "{purpose}: event reported a different pane or status"
            )));
        }
        Ok(())
    }
}

fn append_environment(args: &mut Vec<String>, environment: &[String]) {
    for entry in environment {
        args.extend(["--env".to_owned(), entry.clone()]);
    }
    args.push("--no-focus".to_owned());
}

fn required_object<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    purpose: &str,
) -> Result<&'a Map<String, Value>> {
    map.get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| TermError::failed(format!("{purpose}: '{key}' is not an object")))
}

fn required_string(map: &Map<String, Value>, key: &str, purpose: &str) -> Result<String> {
    match map.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(TermError::failed(format!(
            "{purpose}: '{key}' is not a non-empty string"
        ))),
    }
}

fn optional_string(map: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(TermError::failed(format!("'{key}' is not a string"))),
    }
}

fn object_entries(value: Option<&Value>, purpose: &str) -> Result<Vec<Map<String, Value>>> {
    let Some(Value::Array(items)) = value else {
        return Err(TermError::failed(format!("{purpose}: expected an array")));
    };
    items
        .iter()
        .map(|item| {
            item.as_object()
                .cloned()
                .ok_or_else(|| TermError::failed(format!("{purpose}: entry is not an object")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::CommandOutput;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Scripted {
        calls: Mutex<Vec<Vec<String>>>,
        responses: Mutex<Vec<String>>,
    }

    impl CommandRunner for Scripted {
        fn run(
            &self,
            _program: &std::path::Path,
            args: &[String],
            _stdin: Option<&str>,
        ) -> Result<CommandOutput> {
            self.calls.lock().unwrap().push(args.to_vec());
            let stdout = self
                .responses
                .lock()
                .unwrap()
                .drain(..1)
                .next()
                .unwrap_or_default();
            Ok(CommandOutput {
                status: 0,
                stdout,
                stderr: String::new(),
            })
        }
    }

    fn backend(responses: Vec<&str>) -> (HerdrBackend, Arc<Scripted>) {
        let scripted = Arc::new(Scripted {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into_iter().map(str::to_owned).collect()),
        });
        (
            HerdrBackend::with_runner(PathBuf::from("herdr"), scripted.clone()),
            scripted,
        )
    }

    #[test]
    fn pane_list_and_get_use_the_existing_argument_shapes() {
        let (backend, scripted) = backend(vec![
            r#"{"result":{"panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"}]}}"#,
            r#"{"result":{"pane":{"pane_id":"w1:p1","workspace_id":"w1","cwd":"/work","agent":"codex","agent_status":"idle","terminal_id":"term-9","tab_id":"w1:t1"}}}"#,
        ]);
        let panes = backend.list_panes(Some(&WorkspaceId::from("w1"))).unwrap();
        assert_eq!(panes[0].id.as_str(), "w1:p1");
        let info = backend.pane_info(&PaneId::from("w1:p1")).unwrap();
        assert_eq!(info.agent.as_deref(), Some("codex"));
        assert_eq!(info.agent_status, Some(AgentStatus::Idle));
        let calls = scripted.calls.lock().unwrap();
        assert_eq!(calls[0], strings(&["pane", "list", "--workspace", "w1"]));
        assert_eq!(calls[1], strings(&["pane", "get", "w1:p1"]));
    }

    #[test]
    fn input_uses_send_text_send_keys_and_native_prompt() {
        let (backend, scripted) = backend(vec!["{}", "{}", "{}"]);
        let pane = PaneId::from("w1:p1");
        backend.paste_text(&pane, "hello").unwrap();
        backend.send_keys(&pane, "Enter").unwrap();
        backend.prompt(&pane, "do it").unwrap();
        let calls = scripted.calls.lock().unwrap();
        assert_eq!(calls[0], strings(&["pane", "send-text", "w1:p1", "hello"]));
        assert_eq!(calls[1], strings(&["pane", "send-keys", "w1:p1", "Enter"]));
        assert_eq!(calls[2], strings(&["agent", "prompt", "w1:p1", "do it"]));
    }

    #[test]
    fn tab_create_matches_the_existing_allocation_shape() {
        let (backend, scripted) = backend(vec![
            r#"{"result":{"tab":{"tab_id":"w1:t2"},"root_pane":{"pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1"}}}"#,
        ]);
        let created = backend
            .create_tab(
                &WorkspaceId::from("w1"),
                "worker",
                std::path::Path::new("/tmp"),
                &["A=1".to_owned()],
            )
            .unwrap();
        assert_eq!(created.pane_id.as_str(), "w1:p2");
        assert_eq!(
            scripted.calls.lock().unwrap()[0],
            strings(&[
                "tab",
                "create",
                "--workspace",
                "w1",
                "--label",
                "worker",
                "--cwd",
                "/tmp",
                "--env",
                "A=1",
                "--no-focus"
            ])
        );
    }
}
