//! Follow an observed conversation change only within one pinned native process lifetime.
use super::*;

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    pub(crate) fn follow_observed_session(&self, agent_name: &str) -> Result<bool> {
        let initial = self.load(agent_name)?;
        if initial.harness != "codex"
            || initial.harness_anchor().is_none()
            || initial.lifecycle != "running"
            || initial.paused
            || initial.goal_session_id.is_some()
            || initial.resume.is_some()
            || initial
                .native_session
                .as_ref()
                .is_none_or(|native| native.source != "observed")
        {
            return Ok(false);
        }
        let pane = initial
            .pane_id
            .as_deref()
            .ok_or_else(|| fail("missing pane"))?;
        let info = self.client.pane_info(pane)?;
        if session_matches(&initial, &info) {
            return Ok(false);
        }
        // Observers never wait behind a sender or identity writer.
        let lock = agent::open_private_lock(
            &crate::lock_wait::lock_path(&self.registry, agent_name),
            "agent lifecycle lock",
        )?;
        lock.try_lock_exclusive()
            .map_err(|_| fail("session follow deferred: lifecycle busy"))?;
        let identity =
            agent::open_private_lock(&self.registry.join(".identity.lock"), "agent identity lock")?;
        identity
            .try_lock_exclusive()
            .map_err(|_| fail("session follow deferred: identity busy"))?;
        let mut record = self.load(agent_name)?;
        if record.token != initial.token || record != initial {
            return Err(fail("session follow deferred: record changed"));
        }
        let anchor = record
            .harness_anchor()
            .cloned()
            .ok_or_else(|| fail("missing process pin"))?;
        let session = info
            .session_value
            .as_deref()
            .filter(|id| valid_metadata_text(id))
            .ok_or_else(|| fail("session follow requires a positively observed session"))?;
        if info.session_agent.as_deref() != Some("codex")
            || info.agent.as_deref() != Some("codex")
            || info.terminal_id != record.terminal_id
            || info.workspace_id != record.workspace_id.as_deref().unwrap_or("")
            || !same_directory(&info.cwd, &record.cwd)
            || !self.client.verify_harness_identity(pane, &anchor)?
        {
            return Err(fail("session follow refused: pinned recipient changed"));
        }
        if self
            .identity_owner("codex", session, Some(agent_name))?
            .is_some()
        {
            return Err(fail("session follow refused: conversation already claimed"));
        }
        let old = record.target()?;
        record.session_agent = Some("codex".into());
        record.session_value = Some(session.to_owned());
        record.native_session = Some(NativeSession::new("codex", session, "observed"));
        // Refresh the existing process anchor's spec using supported provenance, and
        // validate it before changing the durable queue binding.
        record.launch_spec =
            self.captured_launch_spec(&record, Some(&anchor), "anchor", None, Some(session))?;
        // Verify unique native routing plus terminal, shell and process pins on the candidate.
        self.checked_policy(&record, false)?;
        if self.client.pane_info(pane)? != info
            || !self.client.verify_harness_identity(pane, &anchor)?
        {
            return Err(fail(
                "session follow refused: recipient moved during verification",
            ));
        }
        let new = record.target()?;
        // Binding first: interruption leaves sends refusing the old record. Repeating this
        // operation accepts either binding and completes the record without replaying input.
        agent::rebind_queue_after_with_runtime(
            &self.queue(agent_name)?,
            &old,
            Some(&new),
            true,
            || Ok((new.clone(), ())),
            &agent::GraceRuntime::new(Duration::from_secs(2)),
        )?;
        self.save(&record)?;
        Ok(true)
    }
}
