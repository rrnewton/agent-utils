//! Planning for fully cultivated workspaces (#1793 agentctl-workspace-cultivation).
//!
//! Pure decisions over observed facts: which workspaces this registry manages,
//! which tabs a hygiene pass would close, rename, create, or promote. Execution
//! (Herdr calls, record writes) lives in `restore::hygiene_pass` and the
//! `agentctl tabs` command; everything here is decided from values, so fakes
//! prove the safety cases without a terminal: a pass never closes a pane with
//! a foreground process other than its shell (`idle_shell` false), never closes
//! a pane whose composer holds unsent text (an unreadable screen counts as
//! holding text), and never closes a pane a registry record points to.

use std::collections::{BTreeMap, BTreeSet};

/// Observed facts about one live pane.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneFacts {
    /// Herdr pane identifier.
    pub pane_id: String,
    /// Herdr tab identifier.
    pub tab_id: String,
    /// Herdr workspace identifier.
    pub workspace_id: String,
    /// The tab's current label, when known.
    pub label: Option<String>,
    /// Kernel proof the pane's only foreground process is its shell.
    pub idle_shell: bool,
    /// The composer holds unsent text; unknown counts as true (fail-closed).
    pub composer_text: bool,
    /// Foreground process display name, for listings.
    pub foreground: String,
    /// The pane's working directory, when known.
    pub cwd: String,
}

/// Observed facts about one registered agent.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentFacts {
    /// The agent's registered name.
    pub name: String,
    /// Its registered pane, when it has one.
    pub pane_id: Option<String>,
    /// Its recorded viewer tab, when named.
    pub viewer_tab_id: Option<String>,
    /// Its recorded lifecycle.
    pub lifecycle: String,
    /// Whether it runs headless (no pane of its own).
    pub headless: bool,
    /// Its recorded working directory.
    pub cwd: String,
    /// The pinned process is proven live as the pane's foreground process.
    pub live_proven: bool,
}

/// One row of an `agentctl tabs` listing.
#[derive(Clone, Debug, PartialEq)]
pub struct TabRow {
    /// Herdr workspace identifier.
    pub workspace_id: String,
    /// Herdr tab identifier.
    pub tab_id: String,
    /// The tab's current label.
    pub label: String,
    /// The registered agent owning the tab, when one does.
    pub owner: Option<String>,
    /// Foreground process display name.
    pub foreground: String,
    /// Seconds the pane has been a bare shell, when tracked.
    pub idle_seconds: Option<f64>,
    /// `keep`, or the reason a pass would close the tab.
    pub verdict: String,
}

/// One action a hygiene pass takes (or, under `--dry-run`, only reports).
#[derive(Clone, Debug, PartialEq)]
pub enum TabAction {
    /// Close one proven bare-shell pane.
    ClosePane {
        /// The pane to close.
        pane_id: String,
        /// Its tab (for the closure record).
        tab_id: String,
        /// Why it closes, in the listing's words.
        why: String,
    },
    /// Close one tab holding no live pane.
    CloseTab {
        /// The tab to close.
        tab_id: String,
        /// Why it closes.
        why: String,
    },
    /// Rename one tab to its agent's name.
    RenameTab {
        /// The tab to rename.
        tab_id: String,
        /// The agent's name it must carry.
        to: String,
    },
    /// Create a missing headless viewer tab.
    CreateViewer {
        /// The headless agent it views.
        agent: String,
        /// The managed workspace it opens in.
        workspace_id: String,
    },
    /// Promote a proven-live `launch_failed` record to `running`.
    Promote {
        /// The agent whose record is corrected.
        agent: String,
    },
}

/// Whether a screen's last nonblank line proves an empty composer: it is a
/// bare prompt (ends with a prompt character) with nothing typed after it.
/// Anything else — text after a prompt, or no prompt at all — counts as
/// holding text, so an unrecognised shell is never closed under its owner.
/// The screen may carry SGR styling (the watch reads styled rows); styling
/// is dropped first, so a coloured prompt that ends in a reset sequence is
/// still recognised as a bare prompt.
#[must_use]
pub fn composer_holds_text(screen: &str) -> bool {
    let plain = crate::submission::join_rows(&crate::submission::render_screen(screen));
    let Some(line) = plain.lines().rev().find(|line| !line.trim().is_empty()) else {
        return false;
    };
    let line = line.trim_end();
    !matches!(line.chars().last(), Some('$' | '#' | '%' | '>' | '❯' | '»'))
}

/// Whether two directories are the same or nested either way: the only
/// corroboration, beyond an exact name match, that ties an unclaimed pane
/// to a registered agent (its recorded cwd, a parent of it, or inside it).
fn cwd_related(left: &str, right: &str) -> bool {
    !left.is_empty()
        && !right.is_empty()
        && (left == right
            || left.starts_with(&format!("{right}/"))
            || right.starts_with(&format!("{left}/")))
}

/// The registered agent a pane duplicates: its tab label is the agent's
/// name AND its cwd is related to the agent's recorded cwd. A label alone
/// never does (#1793 check (1)): an owner's own shell tab that merely
/// shares a label, in an unrelated directory, is not a duplicate.
#[must_use]
pub fn duplicate_of<'a>(pane: &PaneFacts, agents: &'a [AgentFacts]) -> Option<&'a AgentFacts> {
    let label = pane.label.as_deref()?;
    agents
        .iter()
        .find(|agent| agent.name == label && cwd_related(&pane.cwd, &agent.cwd))
}

/// Workspaces this registry manages: holding a claimed pane, a named viewer
/// tab, or a corroborated duplicate pane (name + cwd, never label alone).
#[must_use]
pub fn managed_workspaces(
    panes: &[PaneFacts],
    claimed_panes: &BTreeSet<String>,
    viewer_tabs: &BTreeSet<String>,
    agents: &[AgentFacts],
) -> BTreeSet<String> {
    panes
        .iter()
        .filter(|pane| {
            claimed_panes.contains(&pane.pane_id)
                || viewer_tabs.contains(&pane.tab_id)
                || duplicate_of(pane, agents).is_some()
        })
        .map(|pane| pane.workspace_id.clone())
        .collect()
}

/// Plan one pass. `order_tabs` lists every tab of every workspace (from the
/// workspace tab order); `bare_since`/`now`/`grace_seconds` drive the idle
/// grace with injected time (no wall clock). `viewer_workspace` is where a
/// missing headless viewer is created.
#[must_use]
pub fn plan(
    panes: &[PaneFacts],
    order_tabs: &[(String, String)],
    agents: &[AgentFacts],
    bare_since: &BTreeMap<String, f64>,
    now: f64,
    grace_seconds: f64,
    viewer_workspace: Option<&str>,
) -> (Vec<TabRow>, Vec<TabAction>) {
    let claimed_panes: BTreeSet<String> = agents.iter().filter_map(|a| a.pane_id.clone()).collect();
    let viewer_tabs: BTreeSet<String> = agents
        .iter()
        .filter_map(|a| a.viewer_tab_id.clone())
        .collect();
    let managed = managed_workspaces(panes, &claimed_panes, &viewer_tabs, agents);
    let by_pane: BTreeMap<&str, &AgentFacts> = agents
        .iter()
        .filter_map(|a| Some((a.pane_id.as_deref()?, a)))
        .collect();
    let by_viewer: BTreeMap<&str, &AgentFacts> = agents
        .iter()
        .filter_map(|a| Some((a.viewer_tab_id.as_deref()?, a)))
        .collect();
    let live_tabs: BTreeSet<&str> = panes.iter().map(|p| p.tab_id.as_str()).collect();

    let mut rows = Vec::new();
    let mut actions = Vec::new();
    for pane in panes {
        let owner = by_pane
            .get(pane.pane_id.as_str())
            .or_else(|| by_viewer.get(pane.tab_id.as_str()));
        let idle = bare_since.get(&pane.pane_id).map(|since| now - since);
        let mut verdict = "keep".to_owned();
        if !managed.contains(&pane.workspace_id) {
            verdict = "keep (unmanaged workspace)".to_owned();
        } else if owner.is_none() {
            // Unclaimed: a duplicate only with name+cwd corroboration.
            let duplicate = duplicate_of(pane, agents).is_some();
            if !pane.idle_shell {
                verdict = "keep (foreground process)".to_owned();
            } else if pane.composer_text {
                verdict = "keep (composer text)".to_owned();
            } else if idle.is_some_and(|idle| idle >= grace_seconds) {
                verdict = if duplicate {
                    "close (duplicate idle shell)".to_owned()
                } else {
                    "close (bare idle shell)".to_owned()
                };
                actions.push(TabAction::ClosePane {
                    pane_id: pane.pane_id.clone(),
                    tab_id: pane.tab_id.clone(),
                    why: verdict.clone(),
                });
            } else {
                verdict = "keep (idle grace)".to_owned();
            }
        }
        // Label sync: a claimed or viewer tab carries its agent's name.
        if let Some(agent) = owner {
            if pane.label.as_deref() != Some(agent.name.as_str())
                && managed.contains(&pane.workspace_id)
            {
                actions.push(TabAction::RenameTab {
                    tab_id: pane.tab_id.clone(),
                    to: agent.name.clone(),
                });
            }
        }
        rows.push(TabRow {
            workspace_id: pane.workspace_id.clone(),
            tab_id: pane.tab_id.clone(),
            label: pane.label.clone().unwrap_or_default(),
            owner: owner.map(|agent| agent.name.clone()),
            foreground: pane.foreground.clone(),
            idle_seconds: idle,
            verdict,
        });
    }
    // Orphan tabs: in a managed workspace's order, holding no live pane.
    let managed_tabs: BTreeSet<&str> = order_tabs
        .iter()
        .filter(|(workspace, _)| managed.contains(workspace))
        .map(|(_, tab)| tab.as_str())
        .collect();
    for tab in managed_tabs {
        if !live_tabs.contains(tab) {
            rows.push(TabRow {
                workspace_id: order_tabs
                    .iter()
                    .find(|(_, t)| t == tab)
                    .map_or_else(String::new, |(w, _)| w.clone()),
                tab_id: tab.to_owned(),
                label: String::new(),
                owner: by_viewer.get(tab).map(|agent| agent.name.clone()),
                foreground: String::new(),
                idle_seconds: None,
                verdict: "close (orphan tab, no live pane)".to_owned(),
            });
            actions.push(TabAction::CloseTab {
                tab_id: tab.to_owned(),
                why: "orphan tab, no live pane".to_owned(),
            });
        }
    }
    for agent in agents {
        if agent.lifecycle == "launch_failed" && agent.live_proven {
            actions.push(TabAction::Promote {
                agent: agent.name.clone(),
            });
        }
        // One viewer per live headless agent: missing, closed, or pane-dead heals.
        let viewer_alive = agent
            .viewer_tab_id
            .as_deref()
            .is_some_and(|tab| live_tabs.contains(tab));
        if agent.headless && (agent.lifecycle == "running" || agent.live_proven) && !viewer_alive {
            if let Some(workspace) = viewer_workspace {
                actions.push(TabAction::CreateViewer {
                    agent: agent.name.clone(),
                    workspace_id: workspace.to_owned(),
                });
            }
        }
    }
    (rows, actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, tab: &str, ws: &str, label: &str, idle: bool, composer: bool) -> PaneFacts {
        PaneFacts {
            pane_id: id.into(),
            tab_id: tab.into(),
            workspace_id: ws.into(),
            label: Some(label.into()),
            idle_shell: idle,
            composer_text: composer,
            foreground: if idle { "bash".into() } else { "muse".into() },
            cwd: "/work/slot".into(),
        }
    }
    fn agent(name: &str, pane: Option<&str>) -> AgentFacts {
        AgentFacts {
            name: name.into(),
            pane_id: pane.map(str::to_owned),
            viewer_tab_id: None,
            lifecycle: "running".into(),
            headless: false,
            cwd: "/work/slot".into(),
            live_proven: pane.is_some(),
        }
    }
    fn since(pane: &str, at: f64) -> BTreeMap<String, f64> {
        BTreeMap::from([(pane.to_owned(), at)])
    }

    #[test]
    fn never_closes_a_foreground_process_composer_text_or_registered_pane() {
        // #345's four fakes: foreground process, composer text, registered
        // pane — all kept; only the unregistered idle shell closes.
        let panes = vec![
            pane("w1:p1", "w1:t1", "w1", "agent-a", false, false), // registered, live harness
            pane("w1:p2", "w1:t2", "w1", "scratch", false, false), // foreground process
            pane("w1:p3", "w1:t3", "w1", "scratch", true, true),   // composer text
            pane("w1:p4", "w1:t4", "w1", "scratch", true, false),  // unregistered idle shell
        ];
        let agents = vec![agent("agent-a", Some("w1:p1"))];
        let order: Vec<(String, String)> = panes
            .iter()
            .map(|p| (p.workspace_id.clone(), p.tab_id.clone()))
            .collect();
        let mut bare = BTreeMap::new();
        for p in &panes {
            bare.insert(p.pane_id.clone(), 0.0);
        }
        let (rows, actions) = plan(&panes, &order, &agents, &bare, 10_000.0, 600.0, None);
        assert_eq!(
            actions,
            vec![TabAction::ClosePane {
                pane_id: "w1:p4".into(),
                tab_id: "w1:t4".into(),
                why: "close (bare idle shell)".into()
            }]
        );
        assert!(rows
            .iter()
            .all(|r| r.verdict.starts_with("keep") || r.tab_id == "w1:t4"));
    }

    #[test]
    fn a_duplicate_idle_shell_in_an_unclaimed_workspace_is_managed_and_closed() {
        // Live-proof shape: the real pane lives in w1; w2 holds only a
        // duplicate-labelled bare shell. w2 joins the managed set by that
        // label alone, and the duplicate closes past grace — within grace it keeps.
        let panes = vec![
            pane("w1:p1", "w1:t1", "w1", "agent-a", false, false),
            pane("w2:p9", "w2:t9", "w2", "agent-a", true, false),
        ];
        let agents = vec![agent("agent-a", Some("w1:p1"))];
        let order: Vec<(String, String)> = panes
            .iter()
            .map(|p| (p.workspace_id.clone(), p.tab_id.clone()))
            .collect();
        let (_, actions) = plan(
            &panes,
            &order,
            &agents,
            &since("w2:p9", 0.0),
            10_000.0,
            600.0,
            None,
        );
        assert_eq!(actions.len(), 1);
        let (_, actions) = plan(
            &panes,
            &order,
            &agents,
            &since("w2:p9", 9_900.0),
            10_000.0,
            600.0,
            None,
        );
        assert!(actions.is_empty(), "within grace: {actions:?}");
    }

    #[test]
    fn a_label_alone_never_makes_a_duplicate() {
        // #1793 check (1): an owner's own shell whose tab merely shares an
        // agent's name, in an unrelated directory, is never cultivated.
        let mut foreign = pane("w9:p1", "w9:t1", "w9", "agent-a", true, false);
        foreign.cwd = "/elsewhere".into();
        let panes = vec![
            pane("w1:p1", "w1:t1", "w1", "agent-a", false, false),
            foreign,
        ];
        let agents = vec![agent("agent-a", Some("w1:p1"))];
        let order: Vec<(String, String)> = panes
            .iter()
            .map(|p| (p.workspace_id.clone(), p.tab_id.clone()))
            .collect();
        let (rows, actions) = plan(
            &panes,
            &order,
            &agents,
            &since("w9:p1", 0.0),
            10_000.0,
            600.0,
            None,
        );
        assert!(actions.is_empty(), "{actions:?}");
        assert!(rows
            .iter()
            .find(|r| r.tab_id == "w9:t1")
            .is_some_and(|r| r.verdict.contains("unmanaged")));
    }

    #[test]
    fn an_orphan_tab_closes_and_a_missing_headless_viewer_is_created() {
        let panes = vec![pane("w1:p1", "w1:t1", "w1", "agent-a", false, false)];
        let mut headless = agent("coord", None);
        headless.headless = true;
        let agents = vec![agent("agent-a", Some("w1:p1")), headless];
        let order = vec![
            ("w1".to_owned(), "w1:t1".to_owned()),
            ("w1".to_owned(), "w1:t9".to_owned()), // no pane: orphan
        ];
        let (_, actions) = plan(
            &panes,
            &order,
            &agents,
            &BTreeMap::new(),
            0.0,
            600.0,
            Some("w1"),
        );
        assert!(actions.contains(&TabAction::CloseTab {
            tab_id: "w1:t9".into(),
            why: "orphan tab, no live pane".into()
        }));
        assert!(actions.contains(&TabAction::CreateViewer {
            agent: "coord".into(),
            workspace_id: "w1".into()
        }));
    }

    #[test]
    fn promotion_needs_proven_liveness_and_labels_sync() {
        let panes = vec![pane("w1:p1", "w1:t1", "w1", "old-name", false, false)];
        let mut live = agent("agent-a", Some("w1:p1"));
        live.lifecycle = "launch_failed".into();
        let mut dead = agent("agent-b", None);
        dead.lifecycle = "launch_failed".into();
        dead.live_proven = false;
        let order = vec![("w1".to_owned(), "w1:t1".to_owned())];
        let (_, actions) = plan(
            &panes,
            &order,
            &[live, dead],
            &BTreeMap::new(),
            0.0,
            600.0,
            None,
        );
        assert!(actions.contains(&TabAction::Promote {
            agent: "agent-a".into()
        }));
        assert!(!actions
            .iter()
            .any(|a| matches!(a, TabAction::Promote { agent } if agent == "agent-b")));
        assert!(actions.contains(&TabAction::RenameTab {
            tab_id: "w1:t1".into(),
            to: "agent-a".into()
        }));
    }

    #[test]
    fn composer_detection_fails_closed() {
        assert!(!composer_holds_text("work $ \n"));
        assert!(!composer_holds_text("❯ \n"));
        assert!(composer_holds_text("❯ half-typed message\n"));
        assert!(composer_holds_text("some unrecognised screen\n"));
        assert!(!composer_holds_text(""));
        // A styled two-line bash prompt whose last row ends in an SGR reset.
        assert!(!composer_holds_text(
            "\u{1b}[0m\u{1b}[38;5;4m[user@host /work (main)] \u{1b}[0m\r\n\u{1b}[0m\u{1b}[1m\u{1b}[38;5;6m$ \u{1b}[0m\n"
        ));
        assert!(composer_holds_text(
            "\u{1b}[1m\u{1b}[38;5;6m$ \u{1b}[0mhalf-typed\u{1b}[0m\n"
        ));
    }
}
