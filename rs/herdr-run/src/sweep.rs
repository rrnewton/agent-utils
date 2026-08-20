//! Collect the evidence [`crate::reap`] decides on, from the run spool and the live session.
//!
//! [`crate::reap`] deliberately holds no I/O, so that its policy can be exercised against
//! planted-stale and planted-live populations without a Herdr server. This module is the other
//! half: it gathers the evidence and hands it over. Keeping the two apart is what makes "the reaper
//! spared everything" testable rather than a claim.
//!
//! # Where the candidate set comes from
//!
//! From **our own run spool**, not from `herdr pane list`. Every run herdr-run has performed wrote
//! a `meta.json` naming the pane it used, the workspace and tab labels it resolved, and the agent
//! it ran for. That is a record we authored, so a pane appearing in it is a pane *this tool*
//! opened — which is exactly the population the tab leak is made of. Enumerating live panes instead
//! would sweep up tabs a human opened in the same workspace and make scope a matter of
//! pattern-matching a label.
//!
//! # Scope is re-derived, not read back
//!
//! A recorded tab label is not taken as proof that the tab is ours. The label is recomputed from
//! the CURRENT configuration and the recorded agent name, and must match exactly; the recorded
//! workspace label must equal the configured one. So retargeting `workspace` or `tab_name`
//! immediately takes the old tabs out of scope, rather than leaving this tool authorised over tabs
//! it would no longer create.
//!
//! # What "the shell is gone" means here
//!
//! Herdr still lists the pane, and `pane process-info` still names a shell pid, but `/proc` says no
//! such process exists. That is the husk case: the tab outlived the shell that owned it. Any other
//! outcome — `/proc` unreadable, the pane no longer listed, the control call failing — is UNKNOWN,
//! because only positive absence may authorise closing a tab.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::client::HerdrApi;
use crate::config::Config;
use crate::identity::{current_boot_id, probe_process};
use crate::reap::{plan_reap, PaneEvidence, ProcessIdentity, ReapPlan};
use crate::retention::runs_root;
use crate::session::tab_label_for;

/// Largest `meta.json` accepted. Run metadata is a few kilobytes; the bound keeps one corrupted
/// spool entry from turning a sweep into an unbounded read.
const MAX_META_BYTES: u64 = 1 << 20;

/// Parse every readable `meta.json` under the run spool, oldest run directory first.
///
/// Unreadable or malformed entries are skipped rather than failing the sweep: the spool is
/// operational output, and one truncated file must not stop the report on every other pane.
/// Directory names are timestamp-prefixed, so sorting them orders the records by run time — which
/// is what lets the LAST record for a pane be treated as its current scope.
#[must_use]
pub fn load_run_records(config: &Config) -> Vec<Value> {
    let root = runs_root(
        Path::new(&config.spool_dir),
        Path::new(&config.project_root),
    );
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    names.sort();
    let mut records = Vec::new();
    for directory in names {
        let path = directory.join("meta.json");
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_META_BYTES {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(value) if value.is_object() => records.push(value),
            _ => continue,
        }
    }
    records
}

/// Every distinct pane id named by these records, in first-seen order.
#[must_use]
pub fn pane_ids_in(records: &[Value]) -> Vec<String> {
    let mut seen = Vec::new();
    let mut known = BTreeSet::new();
    for record in records {
        if let Some(pane_id) = record.get("pane_id").and_then(Value::as_str) {
            if !pane_id.is_empty() && known.insert(pane_id.to_owned()) {
                seen.push(pane_id.to_owned());
            }
        }
    }
    seen
}

/// Fold this pane's run records into (exit-code-recorded flags, recorded shell identity).
///
/// A record with `from_cache` true never launched anything, so it is not evidence that a pane
/// exists and is skipped — otherwise a cache hit would manufacture a phantom candidate. Two
/// different recorded shells for one pane id mean the pane was re-incarnated, and neither identity
/// may speak for the other, so the fold reports none.
#[must_use]
pub fn evidence_from_runs(
    pane_id: &str,
    records: &[Value],
) -> (Vec<bool>, Option<ProcessIdentity>) {
    let mut flags = Vec::new();
    let mut identity: Option<ProcessIdentity> = None;
    let mut conflict = false;
    for record in records {
        if record.get("pane_id").and_then(Value::as_str) != Some(pane_id) {
            continue;
        }
        if record.get("from_cache") == Some(&Value::Bool(true)) {
            continue;
        }
        let exit_code = record.get("exit_code");
        flags.push(!matches!(exit_code, None | Some(Value::Null)));
        let Some(readiness) = record.get("readiness").filter(|value| value.is_object()) else {
            continue;
        };
        let Some(shell_pid) = readiness.get("shell_pid").and_then(Value::as_i64) else {
            continue;
        };
        let candidate = ProcessIdentity {
            pid: Some(shell_pid),
            boot_id: readiness
                .get("boot_id")
                .and_then(Value::as_str)
                .filter(|boot| !boot.trim().is_empty())
                .map(ToOwned::to_owned),
            start_ticks: readiness.get("shell_start_ticks").and_then(Value::as_u64),
        };
        match &identity {
            None => identity = Some(candidate),
            Some(existing) if *existing != candidate => conflict = true,
            Some(_) => {}
        }
    }
    (flags, if conflict { None } else { identity })
}

/// Decide scope for one pane: `(in_scope, tab_id, tab_label, workspace_label)`.
///
/// The last record naming the pane wins: a pane id can be reused by a later incarnation, and the
/// most recent run is the only one whose labels describe the tab as it is now.
fn scope_of(
    pane_id: &str,
    records: &[Value],
    config: &Config,
) -> (bool, Option<String>, Option<String>, Option<String>) {
    let Some(latest) = records
        .iter()
        .rfind(|record| record.get("pane_id").and_then(Value::as_str) == Some(pane_id))
    else {
        return (false, None, None, None);
    };
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(ToOwned::to_owned);
    let tab_id = text(latest.pointer("/tab/id"));
    let tab_label = text(latest.pointer("/tab/label"));
    let workspace_label = text(latest.pointer("/workspace/label"));
    let agent = text(latest.get("agent"));

    let matches_schema = match (&workspace_label, &tab_label, &agent) {
        (Some(workspace), Some(label), Some(agent)) if *workspace == config.workspace => {
            tab_label_for(config, agent).is_ok_and(|expected| expected == *label)
        }
        _ => false,
    };
    (matches_schema, tab_id, tab_label, workspace_label)
}

/// Gather one [`PaneEvidence`] per pane this tool has run a command in.
pub fn build_evidence<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    records: Option<Vec<Value>>,
    proc_root: &Path,
) -> Vec<PaneEvidence> {
    let spool = records.unwrap_or_else(|| load_run_records(config));
    let boot = current_boot_id(proc_root);

    let mut live_pane_ids: BTreeSet<String> = BTreeSet::new();
    let mut listing_error: Option<String> = None;
    match client.workspace_id_for_label(&config.workspace) {
        Ok(Some(workspace_id)) => match client.panes(Some(&workspace_id)) {
            Ok(panes) => live_pane_ids.extend(panes.into_iter().map(|pane| pane.pane_id)),
            // Not fatal, and NOT an empty listing: "herdr did not answer" must not be read as
            // "every pane is gone", which is the one mistake that would close the whole workspace.
            Err(error) => listing_error = Some(error.to_string()),
        },
        Ok(None) => {}
        Err(error) => listing_error = Some(error.to_string()),
    }

    let mut evidence = Vec::new();
    for pane_id in pane_ids_in(&spool) {
        let (in_scope, tab_id, tab_label, workspace_label) = scope_of(&pane_id, &spool, config);
        let (flags, recorded) = evidence_from_runs(&pane_id, &spool);
        let known = live_pane_ids.contains(&pane_id);
        let mut live = None;
        let mut error = listing_error.clone();
        if in_scope && error.is_none() && known {
            let (identity, failure) = live_shell(client, &pane_id, boot.as_deref(), proc_root);
            live = identity;
            error = failure;
        }
        evidence.push(PaneEvidence {
            pane_id,
            tab_id,
            tab_label,
            workspace_label,
            in_scope,
            run_exit_codes_recorded: flags,
            recorded_shell: recorded,
            live_shell: live,
            current_boot_id: boot.clone(),
            pane_known_to_herdr: known,
            evidence_error: error,
        });
    }
    evidence
}

/// Bind the pane's CURRENT shell, or say why we could not.
///
/// `None` for the identity means the shell is positively gone — the only reading the policy may
/// turn into a STALE verdict. Everything inconclusive comes back as an error string instead.
fn live_shell<A: HerdrApi + ?Sized>(
    client: &A,
    pane_id: &str,
    boot: Option<&str>,
    proc_root: &Path,
) -> (Option<ProcessIdentity>, Option<String>) {
    let info = match client.process_info(pane_id) {
        Ok(info) => info,
        Err(error) => {
            return (
                None,
                Some(format!("cannot read process info for {pane_id}: {error}")),
            )
        }
    };
    let probe = probe_process(info.shell_pid, proc_root);
    if let Some(error) = probe.error {
        return (None, Some(error));
    }
    if probe.gone {
        return (None, None);
    }
    (
        Some(ProcessIdentity {
            pid: Some(info.shell_pid),
            boot_id: boot.map(ToOwned::to_owned),
            start_ticks: probe.start_ticks,
        }),
        None,
    )
}

/// Gather evidence and run the reaping policy over it. Decides only; closes nothing.
pub fn sweep<A: HerdrApi + ?Sized>(client: &A, config: &Config, proc_root: &Path) -> ReapPlan {
    plan_reap(&build_evidence(client, config, None, proc_root))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use crate::client::{Pane, ProcessInfo};
    use crate::error::{HerdrRunError, Result};
    use crate::reap::Verdict;

    use super::*;

    const BOOT: &str = "3f2b1c8e-0000-4000-8000-000000000001";

    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    /// One live workspace holding chosen panes, each with a chosen shell pid.
    struct SweepFake {
        panes: Vec<Pane>,
        shell_pids: BTreeMap<String, i64>,
        fail_pane_list: bool,
    }

    impl SweepFake {
        fn with_pane(pane_id: &str, shell_pid: i64) -> Self {
            Self {
                panes: vec![Pane {
                    pane_id: pane_id.to_owned(),
                    tab_id: "w1:t1".to_owned(),
                    workspace_id: "w1".to_owned(),
                }],
                shell_pids: BTreeMap::from([(pane_id.to_owned(), shell_pid)]),
                fail_pane_list: false,
            }
        }
    }

    impl HerdrApi for SweepFake {
        fn ensure_server(&self) -> Result<bool> {
            Ok(false)
        }

        fn workspace_id_for_label(&self, _label: &str) -> Result<Option<String>> {
            Ok(Some("w1".to_owned()))
        }

        fn workspace_label_for_id(&self, _workspace_id: &str) -> Result<Option<String>> {
            Ok(Some("agent-cmds".to_owned()))
        }

        fn create_workspace(&self, _label: &str, _cwd: &str) -> Result<(String, String, String)> {
            unreachable!()
        }

        fn tab_id_for_label(&self, _workspace_id: &str, _label: &str) -> Result<Option<String>> {
            Ok(None)
        }

        fn create_tab(&self, _workspace_id: &str, _label: &str, _cwd: &str) -> Result<String> {
            unreachable!()
        }

        fn rename_tab(&self, _tab_id: &str, _label: &str) -> Result<()> {
            Ok(())
        }

        fn panes(&self, _workspace_id: Option<&str>) -> Result<Vec<Pane>> {
            if self.fail_pane_list {
                return Err(HerdrRunError::unavailable(
                    "pane list: herdr is not answering",
                ));
            }
            Ok(self.panes.clone())
        }

        fn pane_exists(&self, pane_id: &str) -> bool {
            self.panes.iter().any(|pane| pane.pane_id == pane_id)
        }

        fn process_info(&self, pane_id: &str) -> Result<ProcessInfo> {
            let shell_pid = *self.shell_pids.get(pane_id).unwrap_or(&100);
            Ok(ProcessInfo {
                pane_id: pane_id.to_owned(),
                shell_pid,
                foreground_pgid: shell_pid,
                foreground: vec![(shell_pid, "bash".to_owned(), "/bin/bash".to_owned())],
            })
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
            "herdr-run-sweep-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temporary root");
        path
    }

    fn config(root: &Path) -> Config {
        Config {
            project_root: root.to_string_lossy().into_owned(),
            spool_dir: "spool".to_owned(),
            ..Config::default()
        }
    }

    fn record(pane_id: &str, exit_code: Value, workspace: &str, agent: &str) -> Value {
        json!({
            "pane_id": pane_id,
            "agent": agent,
            "exit_code": exit_code,
            "workspace": {"label": workspace, "id": "w1"},
            "tab": {"label": agent, "id": "w1:t1"},
            "readiness": {
                "shell_pid": 4242,
                "boot_id": BOOT,
                "shell_start_ticks": 900,
            },
        })
    }

    fn write_spool(root: &Path, records: &[Value]) {
        let runs = root.join("spool").join("runs");
        for (index, value) in records.iter().enumerate() {
            let directory = runs.join(format!("20260819T00000{index}-agent-{index}"));
            fs::create_dir_all(&directory).expect("run directory");
            fs::write(
                directory.join("meta.json"),
                serde_json::to_vec(value).expect("encode"),
            )
            .expect("write meta");
        }
    }

    fn proc_with(root: &Path, pid: Option<i64>, ticks: u64) -> PathBuf {
        let proc = root.join("proc");
        fs::create_dir_all(proc.join("sys/kernel/random")).expect("proc tree");
        fs::write(proc.join(BOOT_ID_PATH_TEST), format!("{BOOT}\n")).expect("boot id");
        if let Some(pid) = pid {
            fs::create_dir_all(proc.join(pid.to_string())).expect("pid directory");
            let padding = vec!["0"; 18].join(" ");
            fs::write(
                proc.join(pid.to_string()).join("stat"),
                format!("{pid} (bash) S {padding} {ticks}\n"),
            )
            .expect("stat");
        }
        proc
    }

    const BOOT_ID_PATH_TEST: &str = crate::identity::BOOT_ID_PATH;

    #[test]
    fn load_run_records_skips_a_corrupt_entry_without_losing_the_rest() {
        let root = temporary_root("corrupt");
        write_spool(
            &root,
            &[
                record("w1:p1", json!(0), "agent-cmds", "kvm"),
                record("w1:p2", json!(0), "agent-cmds", "kvm"),
            ],
        );
        let broken = root.join("spool/runs/20260819T999999-broken");
        fs::create_dir_all(&broken).expect("broken directory");
        fs::write(broken.join("meta.json"), "{not json").expect("write broken");

        let records = load_run_records(&config(&root));
        assert_eq!(pane_ids_in(&records), ["w1:p1", "w1:p2"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_husk_pane_whose_shell_died_is_reaped() {
        // POSITIVE CONTROL. Herdr still lists the tab; the shell that owned it no longer exists.
        let root = temporary_root("husk");
        write_spool(&root, &[record("w1:p1", json!(0), "agent-cmds", "kvm")]);
        let proc = proc_with(&root, None, 0);
        let plan = sweep(&SweepFake::with_pane("w1:p1", 4242), &config(&root), &proc);
        assert_eq!(plan.counts()["STALE"], 1);
        assert_eq!(plan.reapable()[0].pane_id, "w1:p1");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_live_shell_is_spared() {
        let root = temporary_root("live");
        write_spool(&root, &[record("w1:p1", json!(0), "agent-cmds", "kvm")]);
        let proc = proc_with(&root, Some(4242), 900);
        let plan = sweep(&SweepFake::with_pane("w1:p1", 4242), &config(&root), &proc);
        assert_eq!(plan.counts()["SHELL_ALIVE"], 1);
        assert!(plan.reapable().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unfinished_run_beats_a_dead_shell() {
        let root = temporary_root("thinking");
        write_spool(
            &root,
            &[
                record("w1:p1", json!(0), "agent-cmds", "kvm"),
                record("w1:p1", Value::Null, "agent-cmds", "kvm"),
            ],
        );
        let proc = proc_with(&root, None, 0);
        let plan = sweep(&SweepFake::with_pane("w1:p1", 4242), &config(&root), &proc);
        assert_eq!(plan.counts()["IN_FLIGHT"], 1);
        assert_eq!(plan.counts()["STALE"], 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_proc_entry_is_unknown_not_stale() {
        let root = temporary_root("unreadable");
        write_spool(&root, &[record("w1:p1", json!(0), "agent-cmds", "kvm")]);
        let proc = proc_with(&root, None, 0);
        fs::create_dir_all(proc.join("4242")).expect("pid directory");
        fs::write(proc.join("4242").join("stat"), "truncated\n").expect("stat");
        let plan = sweep(&SweepFake::with_pane("w1:p1", 4242), &config(&root), &proc);
        assert_eq!(plan.counts()["UNKNOWN"], 1);
        assert_eq!(plan.counts()["STALE"], 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_tab_retargeted_out_of_the_schema_is_out_of_scope() {
        // Scope is recomputed from the CURRENT config, not read back from the record.
        let root = temporary_root("retargeted");
        write_spool(&root, &[record("w1:p1", json!(0), "agent-cmds", "kvm")]);
        let proc = proc_with(&root, None, 0);
        let mut cfg = config(&root);
        cfg.tab_name = "{project}-{agent}".to_owned();
        let plan = sweep(&SweepFake::with_pane("w1:p1", 4242), &cfg, &proc);
        assert_eq!(plan.counts()["OUT_OF_SCOPE"], 1);
        assert_eq!(plan.counts()["STALE"], 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pane_in_another_workspace_is_out_of_scope() {
        let root = temporary_root("other-workspace");
        write_spool(&root, &[record("w1:p1", json!(0), "someone-elses", "kvm")]);
        let proc = proc_with(&root, None, 0);
        let plan = sweep(&SweepFake::with_pane("w1:p1", 4242), &config(&root), &proc);
        assert_eq!(plan.counts()["OUT_OF_SCOPE"], 1);
        assert!(plan.reapable().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pane_herdr_no_longer_lists_is_unknown() {
        let root = temporary_root("delisted");
        write_spool(
            &root,
            &[
                record("w1:p1", json!(0), "agent-cmds", "kvm"),
                record("w1:pGONE", json!(0), "agent-cmds", "kvm"),
            ],
        );
        let proc = proc_with(&root, Some(4242), 900);
        let plan = sweep(&SweepFake::with_pane("w1:p1", 4242), &config(&root), &proc);
        let verdicts: BTreeMap<&str, Verdict> = plan
            .decisions
            .iter()
            .map(|decision| (decision.pane_id.as_str(), decision.verdict))
            .collect();
        assert_eq!(verdicts["w1:pGONE"], Verdict::Unknown);
        assert_eq!(verdicts["w1:p1"], Verdict::ShellAlive);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unanswering_herdr_reaps_nothing() {
        let root = temporary_root("unanswering");
        write_spool(&root, &[record("w1:p1", json!(0), "agent-cmds", "kvm")]);
        let proc = proc_with(&root, None, 0);
        let mut fake = SweepFake::with_pane("w1:p1", 4242);
        fake.fail_pane_list = true;
        let plan = sweep(&fake, &config(&root), &proc);
        assert_eq!(plan.counts()["STALE"], 0);
        assert_eq!(plan.counts()["UNKNOWN"], 1);
        // And it must say the SERVER did not answer, not that herdr no longer lists the pane. The
        // verdict is the same either way, but the second sentence tells an operator the tabs are
        // already gone -- which is the opposite of what happened.
        assert!(
            plan.declined()[0].reason.contains("not answering"),
            "{}",
            plan.declined()[0].reason
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_empty_spool_reports_zero_of_everything() {
        let root = temporary_root("empty");
        let proc = proc_with(&root, None, 0);
        let plan = sweep(&SweepFake::with_pane("w1:p1", 4242), &config(&root), &proc);
        let counts = plan.counts();
        assert_eq!(counts["considered"], 0);
        for verdict in Verdict::all() {
            assert_eq!(counts[verdict.as_str()], 0);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn evidence_carries_the_recorded_identity_the_policy_needs() {
        let root = temporary_root("identity");
        write_spool(&root, &[record("w1:p1", json!(0), "agent-cmds", "kvm")]);
        let proc = proc_with(&root, Some(4242), 900);
        let evidence = build_evidence(
            &SweepFake::with_pane("w1:p1", 4242),
            &config(&root),
            None,
            &proc,
        );
        assert_eq!(evidence.len(), 1);
        let recorded = evidence[0].recorded_shell.as_ref().expect("recorded shell");
        assert!(
            recorded.is_bound(),
            "a bare pid can never authorise anything"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_cache_hit_does_not_manufacture_a_candidate() {
        let mut cached = record("w1:p1", json!(0), "agent-cmds", "kvm");
        cached["from_cache"] = json!(true);
        let (flags, identity) = evidence_from_runs("w1:p1", &[cached]);
        assert!(flags.is_empty());
        assert!(identity.is_none());
    }

    #[test]
    fn conflicting_pane_shell_incarnations_forfeit_the_identity() {
        let mut first = record("w1:p1", json!(0), "agent-cmds", "kvm");
        first["readiness"]["shell_pid"] = json!(100);
        let mut second = record("w1:p1", json!(0), "agent-cmds", "kvm");
        second["readiness"]["shell_pid"] = json!(200);
        let (flags, identity) = evidence_from_runs("w1:p1", &[first, second]);
        assert_eq!(flags, [true, true]);
        assert!(
            identity.is_none(),
            "a recycled pane id must not inherit the first shell's authority"
        );
    }
}
