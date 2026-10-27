//! Operator-authorised retirement of stale control metadata, never a runtime stop.
use super::*;

/// Exact-generation assertions for archiving a stale interactive registration.
#[derive(Clone, Debug)]
pub struct RetireStaleOptions {
    /// Current generation token, from status or the preserved raw record.
    pub expected_token: String,
    /// SHA-256 of the exact raw agent.json bytes.
    pub expected_record_sha256: String,
    /// SHA-256 of an incomplete revive journal; absent means do not retire a revive.
    pub expected_revive_sha256: Option<String>,
    /// Explicitly selected live candidate pane to preserve when retiring a revive.
    pub replacement_pane: Option<String>,
    /// Native session independently observed in that candidate pane.
    pub replacement_session: Option<String>,
    /// Show the checked plan without archiving metadata.
    pub dry_run: bool,
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    fn stale_registration_proof(&self, record: &AgentRecord) -> Result<Value> {
        let panes = self.client.panes()?;
        let matches: Vec<_> = panes
            .iter()
            .filter(|pane| Some(&pane.pane_id) == record.pane_id.as_ref())
            .collect();
        if matches.len() > 1 {
            return Err(fail("pane census is ambiguous"));
        }
        let Some(pane) = matches.first() else {
            return Ok(json!({"kind":"pane-missing"}));
        };
        let info = self.client.pane_info(&pane.pane_id)?;
        let terminal_changed = record
            .terminal_id
            .as_ref()
            .is_some_and(|old| info.terminal_id.as_ref().is_some_and(|new| new != old));
        let session_changed = record
            .observed_session_value()
            .is_some_and(|old| info.session_value.as_deref().is_some_and(|new| new != old));
        let harness_changed = info
            .agent
            .as_deref()
            .is_some_and(|kind| kind != record.harness);
        if !terminal_changed && !session_changed && !harness_changed {
            return Err(fail("registration still matches its live pane; use normal stop for a valid registration"));
        }
        Ok(json!({"kind":"replaced-presentation", "pane": info.pane_id,
            "terminal": info.terminal_id, "session": info.session_value, "harness": info.agent}))
    }

    /// Archive stale metadata under exact generation/byte pins. Never send input, signal a
    /// process, close a pane, or replay queued work. A retry finishes an interrupted archive.
    pub fn retire_stale_registration(
        &self,
        agent_name: &str,
        options: &RetireStaleOptions,
    ) -> Result<Value> {
        name(agent_name)?;
        if !revive::valid_digest(&options.expected_record_sha256)
            || options
                .expected_revive_sha256
                .as_deref()
                .is_some_and(|sha| !revive::valid_digest(sha))
        {
            return Err(fail(
                "expected SHA-256 must be 64 lowercase hexadecimal digits",
            ));
        }
        let _lock = self.lock(agent_name)?;
        let _identity = self.identity_lock()?;
        if self
            .rename_journals()?
            .iter()
            .any(|journal| journal.names(&[agent_name]))
        {
            return Err(fail(
                "registration has an incomplete rename; finish the rename before retirement",
            ));
        }
        let source = self.directory(agent_name)?;
        let archive = self.registry.join("archive");
        let destination = archive.join(format!(
            "{agent_name}-stale-{}",
            options.expected_record_sha256
        ));
        let already_archived = destination.try_exists().map_err(|e| fail(e.to_string()))?;
        let (pinned, record, bytes) = revive::read_record_at(
            if already_archived {
                &destination
            } else {
                &source
            },
            agent_name,
        )?;
        if !revive::token(&record.token)
            || record.token != options.expected_token
            || format!("{:x}", Sha256::digest(&bytes)) != options.expected_record_sha256
        {
            return Err(fail(
                "registration generation or raw record SHA-256 changed; nothing retired",
            ));
        }
        if already_archived && source.try_exists().map_err(|e| fail(e.to_string()))? {
            let (_, current, _) = revive::read_record_at(&source, agent_name)?;
            if current.token == options.expected_token {
                return Err(fail("both current and archived old registration exist; refusing ambiguous retirement"));
            }
        }
        if record.mode != "interactive"
            || record.backend != "herdr"
            || !matches!(
                record.adapter.as_str(),
                "herdr" | "herdr-foreign" | "herdr-pane"
            )
        {
            return Err(fail(
                "stale registration retirement supports interactive Herdr panes only",
            ));
        }
        let proof = if already_archived {
            json!({"kind":"already-archived"})
        } else {
            self.stale_registration_proof(&record)?
        };
        // Preflight the journal before the registration moves. Resume can find it beside the
        // archive if the previous invocation moved the journal before its operation directory.
        let transaction = self.retire_revive_preflight(&record, &destination, options)?;
        let report = json!({"name":agent_name, "archive":destination, "proof":proof,
            "runtime_preserved":true, "pane_closed":false, "tab_closed":false,
            "dry_run":options.dry_run, "already_archived":already_archived,
            "revive_retired":transaction.is_some()});
        if options.dry_run {
            return Ok(report);
        }
        if !already_archived {
            let _queues = self.queue_locks(agent_name)?;
            agent::create_private_directory(&archive, "agent archive", false, false)?;
            if self.stale_registration_proof(&record)? != proof {
                return Err(fail(
                    "live presentation changed during retirement; nothing retired",
                ));
            }
            self.publish_pinned_directory(&pinned, &destination, &bytes)?;
        }
        if let Some((journal, operation)) = transaction {
            self.retire_revive_archive(&record, &destination, &journal, &operation, options).map_err(|error| fail(format!("registration archived at {}; revive archival incomplete: {error}; rerun with the same pins", destination.display())))?;
        }
        agent::sync_directory(&archive)?;
        agent::sync_directory(source.parent().unwrap_or(&self.registry))?;
        Ok(report)
    }
}
