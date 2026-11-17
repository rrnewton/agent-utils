//! A restart's slot enrollment is intent, not a mutation of the live registration.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Enrollment {
    source_sha256: String,
    slot: String,
    project: String,
}

fn fingerprint(record: &AgentRecord) -> Result<String> {
    // Revive's durable stopped snapshot changes this field only. Recovery compares
    // the same source policy both before and after that journaled transition.
    let mut source = record.clone();
    source.lifecycle = "running".to_owned();
    Ok(digest(&document_bytes(&source)?))
}

impl Enrollment {
    pub(super) fn target(&self, old: &AgentRecord) -> Result<AgentRecord> {
        if !matches!(old.lifecycle.as_str(), "running" | "stopped")
            || self.source_sha256 != fingerprint(old)?
            || old.slot.is_some()
            || old.slot_project.is_some()
            || old.slot_isolation.is_some()
            || old.extra.contains_key("project_box")
            || old.is_foreign()
            || old.agentcloud.is_some()
            || !Path::new(&self.project).is_absolute()
        {
            return Err(fail(
                "restart slot enrollment source changed or is not an owned unboxed lane",
            ));
        }
        if !valid_metadata_text(&self.slot) {
            return Err(fail("invalid enrollment slot name"));
        }
        let mut target = old.clone();
        target.slot = Some(self.slot.clone());
        target.slot_project = Some(self.project.clone());
        target.slot_isolation = Some("cgroup".to_owned());
        if let Some(spec) = target.launch_spec.as_mut() {
            spec["slot"] = json!(self.slot);
            spec["slot_project"] = json!(self.project);
            spec["slot_isolation"] = json!("cgroup");
        }
        Ok(target)
    }
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    pub(super) fn restart_enrollment(&self, old: &AgentRecord) -> Result<Option<Enrollment>> {
        let path = self
            .registry
            .join(".restarts")
            .join(format!("{}.json", old.token));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(fail(error.to_string())),
            Ok(_) => {}
        }
        // Old, non-enrollment restart journals retain their existing compatibility rules.
        // Every enrollment read uses the private-file checks, including during recovery.
        let doc = agent_queue::fsio::read_json(&path).map_err(|error| fail(error.to_string()))?;
        if doc.get("slot_enrollment").is_none_or(Value::is_null) {
            return Ok(None);
        }
        agent::validate_private_directory(
            path.parent().expect("restart parent"),
            "restart journals",
            false,
        )?;
        let doc = agent::read_private_json(&path)?;
        if doc["schema"] != "agentctl-restart/v1"
            || doc["name"] != old.name
            || doc["old_token"] != old.token
            || doc["session"] != revive_conversation(old)?.value
            || !matches!(doc["phase"].as_str(), Some("planned" | "exited"))
        {
            return Err(fail("restart slot enrollment journal identity is invalid"));
        }
        let enrollment: Enrollment = serde_json::from_value(doc["slot_enrollment"].clone())
            .map_err(|error| fail(format!("invalid restart slot enrollment: {error}")))?;
        enrollment.target(old)?;
        Ok(Some(enrollment))
    }

    pub(super) fn enrolled_target(&self, old: &AgentRecord) -> Result<AgentRecord> {
        match self.restart_enrollment(old)? {
            Some(enrollment) => enrollment.target(old),
            None => Ok(old.clone()),
        }
    }

    pub(super) fn prepare_enrollment(
        &self,
        old: &AgentRecord,
        slot: &SlotLaunch,
    ) -> Result<Enrollment> {
        if slot.isolation.as_deref() != Some("cgroup")
            || slot.project_box
            || slot.write_sandbox
            || slot.box_cwd.is_some()
            || slot.box_writable.is_some()
            || slot.executable.is_some()
        {
            return Err(fail("restart enrollment requires --slot-isolation cgroup and the configured slot manager"));
        }
        let project = slot
            .project
            .as_ref()
            .ok_or_else(|| fail("restart enrollment requires --slot-project"))?;
        let project =
            fs::canonicalize(project).map_err(|error| fail(format!("slot project: {error}")))?;
        let enrollment = Enrollment {
            source_sha256: fingerprint(old)?,
            slot: slot.slot.clone(),
            project: project.to_string_lossy().into_owned(),
        };
        self.revive_launch_plan(old, Duration::from_secs(30), old.pane_id.clone())?;
        let target = enrollment.target(old)?;
        // The existing launch-plan protocol checks that the slot still resolves to this exact
        // cwd and cgroup isolation. Enrollment never moves a conversation to another checkout.
        self.revive_launch_plan(&target, Duration::from_secs(30), None)?;
        Ok(enrollment)
    }

    pub(super) fn bind_enrolled_candidate(
        &self,
        candidate: &AgentRecord,
        old: &AgentRecord,
    ) -> Result<()> {
        if self.restart_enrollment(old)?.is_none() {
            return Ok(());
        }
        #[cfg(test)]
        let wrkslots = self.revive_wrkslots_executable.clone();
        #[cfg(not(test))]
        let wrkslots = configured_wrkslots(&self.registry)?.or_else(wrkslots_executable);
        let wrkslots = wrkslots.ok_or_else(|| fail("enrollment slot manager unavailable"))?;
        let pid = candidate
            .harness_anchor()
            .or(candidate.custom_process_identity.as_ref())
            .ok_or_else(|| fail("enrollment candidate has no pinned harness"))?
            .pid;
        // transfer-owner checks placement and generation. A refusal leaves the old registration
        // and the live staged candidate intact; revive retries this bind without launching twice.
        crate::restore::bind_slot_owner(
            &crate::restore::SystemHost,
            &wrkslots,
            Path::new(
                candidate
                    .slot_project
                    .as_deref()
                    .ok_or_else(|| fail("missing slot project"))?,
            ),
            candidate
                .slot
                .as_deref()
                .ok_or_else(|| fail("missing slot"))?,
            &candidate.name,
            pid,
        )
        .map_err(|error| {
            fail(format!(
                "restart enrollment owner bind refused before retirement: {error}"
            ))
        })?;
        Ok(())
    }
}
