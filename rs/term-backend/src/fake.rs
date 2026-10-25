//! An in-memory backend for tests: deterministic ids, recorded input,
//! and a scriptable screen per pane.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::{
    Capabilities, CreatedPane, Multiplexer, PaneId, PaneInfo, PaneTerminal, Result, ScreenRead,
    TabId, TabInfo, TermError, WorkspaceId, WorkspaceInfo,
};

#[derive(Default)]
struct State {
    workspaces: Vec<WorkspaceInfo>,
    tabs: Vec<TabInfo>,
    panes: Vec<PaneInfo>,
    screens: BTreeMap<String, String>,
    pasted: Vec<(String, String)>,
    keys: Vec<(String, String)>,
    focused_panes: Vec<String>,
    closed_panes: Vec<String>,
    next: u64,
}

/// In-memory [`crate::TermBackend`] for tests of backend-neutral callers.
pub struct FakeBackend {
    state: Mutex<State>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeBackend {
    /// An empty backend with no workspaces.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
        }
    }

    /// Set the screen text `read_screen` returns for one pane.
    pub fn set_screen(&self, pane: &PaneId, text: &str) {
        self.state
            .lock()
            .unwrap()
            .screens
            .insert(pane.to_string(), text.to_owned());
    }

    /// Every `(pane, text)` pasted so far, in order.
    pub fn pasted(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().pasted.clone()
    }

    /// Every `(pane, keys)` sent so far, in order.
    pub fn sent_keys(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().keys.clone()
    }

    /// Every pane focused so far, in order.
    pub fn focused(&self) -> Vec<String> {
        self.state.lock().unwrap().focused_panes.clone()
    }

    /// Every pane closed so far, in order.
    pub fn closed(&self) -> Vec<String> {
        self.state.lock().unwrap().closed_panes.clone()
    }

    fn allocate(
        &self,
        workspace: Option<WorkspaceId>,
        label: &str,
        cwd: &std::path::Path,
    ) -> CreatedPane {
        let mut state = self.state.lock().unwrap();
        state.next += 1;
        let n = state.next;
        let workspace_id = workspace.unwrap_or_else(|| WorkspaceId::from(format!("fake-w{n}")));
        let tab_id = TabId::from(format!("fake-t{n}"));
        let pane_id = PaneId::from(format!("fake-p{n}"));
        if !state
            .workspaces
            .iter()
            .any(|entry| entry.id == workspace_id)
        {
            state.workspaces.push(WorkspaceInfo {
                id: workspace_id.clone(),
                label: label.to_owned(),
            });
        }
        state.tabs.push(TabInfo {
            id: tab_id.clone(),
            workspace_id: workspace_id.clone(),
            label: label.to_owned(),
        });
        state.panes.push(PaneInfo {
            id: pane_id.clone(),
            tab_id: Some(tab_id.clone()),
            workspace_id: Some(workspace_id.clone()),
            cwd: Some(cwd.to_path_buf()),
            title: None,
            agent: None,
            agent_status: None,
            pid: None,
            terminal_id: None,
        });
        CreatedPane {
            workspace_id,
            tab_id,
            pane_id,
        }
    }

    fn require_pane(&self, pane: &PaneId) -> Result<PaneInfo> {
        self.state
            .lock()
            .unwrap()
            .panes
            .iter()
            .find(|info| &info.id == pane)
            .cloned()
            .ok_or_else(|| TermError::NotFound(format!("fake pane {pane} not found")))
    }
}

impl Multiplexer for FakeBackend {
    fn backend_name(&self) -> &'static str {
        "fake"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            native_agent_status: false,
            native_prompt: false,
        }
    }

    fn list_workspaces(&self) -> Result<Vec<WorkspaceInfo>> {
        Ok(self.state.lock().unwrap().workspaces.clone())
    }

    fn create_workspace(
        &self,
        label: &str,
        cwd: &std::path::Path,
        _environment: &[String],
    ) -> Result<CreatedPane> {
        Ok(self.allocate(None, label, cwd))
    }

    fn focus_workspace(&self, _workspace: &WorkspaceId) -> Result<()> {
        Ok(())
    }

    fn rename_workspace(&self, workspace: &WorkspaceId, label: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        match state
            .workspaces
            .iter_mut()
            .find(|entry| &entry.id == workspace)
        {
            Some(entry) => {
                entry.label = label.to_owned();
                Ok(())
            }
            None => Err(TermError::NotFound(format!(
                "fake workspace {workspace} not found"
            ))),
        }
    }

    fn close_workspace(&self, workspace: &WorkspaceId) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.workspaces.retain(|entry| &entry.id != workspace);
        state.tabs.retain(|entry| &entry.workspace_id != workspace);
        state
            .panes
            .retain(|entry| entry.workspace_id.as_ref() != Some(workspace));
        Ok(())
    }

    fn list_tabs(&self, workspace: &WorkspaceId) -> Result<Vec<TabInfo>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .tabs
            .iter()
            .filter(|entry| &entry.workspace_id == workspace)
            .cloned()
            .collect())
    }

    fn create_tab(
        &self,
        workspace: &WorkspaceId,
        label: &str,
        cwd: &std::path::Path,
        _environment: &[String],
    ) -> Result<CreatedPane> {
        Ok(self.allocate(Some(workspace.clone()), label, cwd))
    }

    fn focus_tab(&self, _tab: &TabId) -> Result<()> {
        Ok(())
    }

    fn rename_tab(&self, tab: &TabId, label: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        match state.tabs.iter_mut().find(|entry| &entry.id == tab) {
            Some(entry) => {
                entry.label = label.to_owned();
                Ok(())
            }
            None => Err(TermError::NotFound(format!("fake tab {tab} not found"))),
        }
    }

    fn close_tab(&self, tab: &TabId) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.tabs.retain(|entry| &entry.id != tab);
        state
            .panes
            .retain(|entry| entry.tab_id.as_ref() != Some(tab));
        Ok(())
    }

    fn list_panes(&self, workspace: Option<&WorkspaceId>) -> Result<Vec<PaneInfo>> {
        let state = self.state.lock().unwrap();
        Ok(state
            .panes
            .iter()
            .filter(|info| workspace.is_none_or(|w| info.workspace_id.as_ref() == Some(w)))
            .cloned()
            .collect())
    }

    fn focus_pane(&self, pane: &PaneId) -> Result<()> {
        self.require_pane(pane)?;
        self.state
            .lock()
            .unwrap()
            .focused_panes
            .push(pane.to_string());
        Ok(())
    }

    fn close_pane(&self, pane: &PaneId) -> Result<()> {
        self.require_pane(pane)?;
        let mut state = self.state.lock().unwrap();
        state.panes.retain(|info| &info.id != pane);
        state.closed_panes.push(pane.to_string());
        Ok(())
    }
}

impl PaneTerminal for FakeBackend {
    fn pane_info(&self, pane: &PaneId) -> Result<PaneInfo> {
        self.require_pane(pane)
    }

    fn read_screen(&self, pane: &PaneId, _read: ScreenRead) -> Result<String> {
        self.require_pane(pane)?;
        Ok(self
            .state
            .lock()
            .unwrap()
            .screens
            .get(pane.as_str())
            .cloned()
            .unwrap_or_default())
    }

    fn paste_text(&self, pane: &PaneId, text: &str) -> Result<()> {
        self.require_pane(pane)?;
        self.state
            .lock()
            .unwrap()
            .pasted
            .push((pane.to_string(), text.to_owned()));
        Ok(())
    }

    fn send_keys(&self, pane: &PaneId, keys: &str) -> Result<()> {
        self.require_pane(pane)?;
        self.state
            .lock()
            .unwrap()
            .keys
            .push((pane.to_string(), keys.to_owned()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn fake_round_trips_through_the_traits() {
        let backend = FakeBackend::new();
        let created = backend
            .create_workspace("project", Path::new("/work"), &[])
            .unwrap();
        assert_eq!(backend.list_workspaces().unwrap()[0].label, "project");
        assert_eq!(backend.list_panes(None).unwrap().len(), 1);
        backend.set_screen(&created.pane_id, "ready\n");
        assert_eq!(
            backend
                .read_screen(&created.pane_id, ScreenRead::visible())
                .unwrap(),
            "ready\n"
        );
        backend.prompt(&created.pane_id, "hello").unwrap();
        assert_eq!(
            backend.pasted(),
            [(created.pane_id.to_string(), "hello".to_owned())]
        );
        assert_eq!(
            backend.sent_keys(),
            [(created.pane_id.to_string(), "Enter".to_owned())]
        );
        assert!(backend.pane_info(&PaneId::from("nope")).is_err());
    }
}
