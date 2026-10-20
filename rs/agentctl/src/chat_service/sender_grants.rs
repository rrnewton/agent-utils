//! Operator commands for explicit, time-limited sender authority.

use super::*;
use crate::chat_runtime::SenderDirectory;

const ID_HELP: &str = "Use authenticated provider sender IDs. For Slack, find a member's U… ID with slack.user describe --user <unixname|email|ID>, or use a helper supporting resolve_sender.";

/// Grant explicit senders, or the current explicitly enumerated members of every configured
/// channel, permission to create requests for a bounded duration.
pub fn grant(
    root: &Path,
    senders: &[String],
    all_members: bool,
    duration: Duration,
    note: Option<&str>,
) -> Result<Value, ChatServiceError> {
    let state = BridgeState::inspect(root)?;
    if duration > Duration::from_secs(state.config().max_grant_seconds) {
        return Err(ChatRuntimeError::Invalid(format!(
            "requested grant duration is {}s, exceeding max_grant_seconds={}s; choose a shorter --for or use chat grant-limit to change it",
            duration.as_secs(), state.config().max_grant_seconds,
        )).into());
    }
    let needs_directory = all_members || senders.iter().any(|sender| !explicit_id(sender));
    let mut helper = if needs_directory {
        state.outbound_transport()?
    } else {
        None
    };
    grant_with_directory(
        &state,
        senders,
        all_members,
        duration,
        note,
        helper
            .as_mut()
            .map(|helper| helper as &mut dyn SenderDirectory),
    )
}

fn grant_with_directory(
    state: &BridgeState,
    senders: &[String],
    all_members: bool,
    duration: Duration,
    note: Option<&str>,
    directory: Option<&mut dyn SenderDirectory>,
) -> Result<Value, ChatServiceError> {
    if all_members != senders.is_empty() {
        return Err(ChatRuntimeError::Invalid(
            "provide sender IDs or --all-members, exclusively".to_owned(),
        )
        .into());
    }
    let ids = if all_members {
        let directory = directory.ok_or_else(|| {
            ChatRuntimeError::Invalid(format!(
                "--all-members requires an outbound helper supporting list_members. {ID_HELP}"
            ))
        })?;
        let mut ids = BTreeSet::new();
        // Finish every listing before writing any grant. An incomplete channel enumeration
        // cannot silently grant only the first page or the channels that happened to work.
        for channel in &state.config().channel_ids {
            ids.extend(directory.list_members(channel)?);
        }
        ids.into_iter().collect::<Vec<_>>()
    } else {
        resolve_inputs(state, senders, directory)?
    };
    let config = state.config();
    let slack = config.subscription_plugin == "slack"
        || config
            .backend_configuration
            .as_ref()
            .is_some_and(|configuration| configuration.schema == "agentctl.slack.v1");
    if slack {
        if let Some(sender) = ids.iter().find(|sender| !slack_sender_id(sender)) {
            return Err(ChatRuntimeError::Invalid(format!(
                "cannot grant unresolved or invalid Slack sender {sender:?}; supply a U… or W… provider ID, or configure an outbound helper supporting resolve_sender. {ID_HELP}"
            )).into());
        }
    }
    let saved = state.grant_senders(&ids, duration, note)?;
    let owners = ids
        .iter()
        .filter(|sender| state.config().allowed_senders.contains(sender))
        .collect::<Vec<_>>();
    Ok(json!({
        "grants": saved,
        "permanent_owners": owners,
        "provider_refresh": "next provider item or heartbeat; no service restart required",
        "sender_id_help": ID_HELP,
    }))
}

// Provider IDs are opaque in the neutral contract. Recognize conventional resource IDs and
// Slack user IDs so they work even with helpers that predate directory operations. With no
// directory helper, the caller supplies raw IDs; the command's output explains how to find them.
fn explicit_id(value: &str) -> bool {
    value.contains('/') || slack_sender_id(value)
}

fn slack_sender_id(value: &str) -> bool {
    matches!(value.as_bytes().first(), Some(b'U' | b'W'))
        && (9..=24).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn resolve_inputs(
    state: &BridgeState,
    senders: &[String],
    mut directory: Option<&mut dyn SenderDirectory>,
) -> Result<Vec<String>, ChatServiceError> {
    let retained = state.grants()?;
    let mut ids = BTreeSet::new();
    for sender in senders {
        let known = explicit_id(sender)
            || state.config().allowed_senders.contains(sender)
            || retained.iter().any(|grant| grant.sender_id == *sender);
        let id = match directory.as_mut() {
            Some(directory) if !known => directory
                .resolve_sender(&state.config().channel_ids[0], sender)
                .map_err(|error| {
                    ChatRuntimeError::Invalid(format!(
                        "cannot resolve sender {sender:?}: {error}. {ID_HELP}"
                    ))
                })?,
            _ => sender.clone(),
        };
        ids.insert(id);
    }
    Ok(ids.into_iter().collect())
}

/// Remove temporary grants. Permanent owners cannot be revoked by this command.
pub fn revoke(root: &Path, senders: &[String]) -> Result<Value, ChatServiceError> {
    let state = BridgeState::inspect(root)?;
    let mut helper = if senders.iter().any(|sender| !explicit_id(sender)) {
        state.outbound_transport()?
    } else {
        None
    };
    let ids = resolve_inputs(
        &state,
        senders,
        helper
            .as_mut()
            .map(|helper| helper as &mut dyn SenderDirectory),
    )?;
    Ok(json!({"revoked": state.revoke_senders(&ids)?}))
}

/// Change the configured maximum duration without initializing the bridge again.
pub fn grant_limit(root: &Path, duration: Duration) -> Result<Value, ChatServiceError> {
    let state = BridgeState::inspect(root)?;
    Ok(json!({"max_grant_seconds": state.set_max_grant_duration(duration)?}))
}

/// List active and expired grants, including their creation, expiry and time remaining.
/// This command only reads durable state and never contacts a provider or Herdr.
pub fn grants(root: &Path, as_json: bool) -> Result<String, ChatServiceError> {
    let state = BridgeState::inspect(root)?;
    let now = chat_runtime::unix_millis();
    let grants = state.grants()?;
    if as_json {
        let entries = grants
            .iter()
            .map(|grant| {
                let mut item = serde_json::to_value(grant).expect("a sender grant serializes");
                item["status"] = json!(if now < grant.created_at_millis {
                    "pending"
                } else if now < grant.expires_at_millis {
                    "active"
                } else {
                    "expired"
                });
                item["remaining_seconds"] =
                    json!(grant.expires_at_millis.saturating_sub(now).div_ceil(1_000));
                item["starts_in_seconds"] =
                    json!(grant.created_at_millis.saturating_sub(now).div_ceil(1_000));
                item
            })
            .collect::<Vec<_>>();
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "owners": state.config().allowed_senders,
                "max_grant_seconds": state.config().max_grant_seconds,
                "grants": entries,
            }))
            .map_err(ChatRuntimeError::from)?
        ));
    }
    let mut text = format!(
        "Maximum grant: {} seconds\n",
        state.config().max_grant_seconds
    );
    for owner in &state.config().allowed_senders {
        text.push_str(&format!("{owner}\tpermanent owner\n"));
    }
    for grant in grants {
        let remaining = if now < grant.created_at_millis {
            format!(
                "pending; starts in {}s",
                (grant.created_at_millis - now).div_ceil(1_000)
            )
        } else if now < grant.expires_at_millis {
            format!(
                "active; {}s remaining",
                (grant.expires_at_millis - now).div_ceil(1_000)
            )
        } else {
            format!(
                "expired {}s ago; 0s remaining",
                (now - grant.expires_at_millis) / 1_000
            )
        };
        text.push_str(&format!(
            "{}\t{remaining}\tcreated={} expires={} Unix ms{}\n",
            grant.sender_id,
            grant.created_at_millis,
            grant.expires_at_millis,
            grant
                .note
                .map_or(String::new(), |note| format!("\tnote={note}")),
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicU64;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        root: PathBuf,
        state: BridgeState,
    }

    impl Scratch {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentctl-grant-directory-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&root).expect("scratch root");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root");
            let config = serde_json::from_value(json!({
                "subscription_plugin": "fixture",
                "channel_ids": ["spaces/first", "spaces/second"],
                "allowed_senders": ["users/owner"],
                "agent_name": "coordinator", "agent_label": "coordinator",
                "outbound_enabled": false, "ack_reaction": null,
            }))
            .expect("configuration");
            let state = BridgeState::initialize(&root.join("state"), config).expect("state");
            Self { root, state }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[derive(Default)]
    struct Directory {
        members: BTreeMap<String, Vec<String>>,
        listed: Vec<String>,
        resolved: Vec<(String, String)>,
    }

    impl SenderDirectory for Directory {
        fn resolve_sender(
            &mut self,
            channel: &str,
            query: &str,
        ) -> Result<String, OutboundFailure> {
            self.resolved.push((channel.to_owned(), query.to_owned()));
            Ok("users/resolved".to_owned())
        }

        fn list_members(&mut self, channel: &str) -> Result<Vec<String>, OutboundFailure> {
            self.listed.push(channel.to_owned());
            self.members
                .get(channel)
                .cloned()
                .ok_or_else(|| OutboundFailure {
                    code: "incomplete_listing".to_owned(),
                    detail: "fixture listing failed".to_owned(),
                    outcome: chat_runtime::OutboundOutcome::NotApplied,
                    retryable: false,
                })
        }
    }

    #[test]
    fn all_members_records_each_current_id_explicitly_and_never_covers_a_later_joiner() {
        let scratch = Scratch::new();
        let mut directory = Directory {
            members: BTreeMap::from([
                (
                    "spaces/first".to_owned(),
                    vec!["users/owner".to_owned(), "users/first".to_owned()],
                ),
                (
                    "spaces/second".to_owned(),
                    vec!["users/first".to_owned(), "users/second".to_owned()],
                ),
            ]),
            ..Directory::default()
        };
        let result = grant_with_directory(
            &scratch.state,
            &[],
            true,
            Duration::from_secs(43_200),
            Some("channel discussion"),
            Some(&mut directory),
        )
        .expect("grant current members");
        assert_eq!(directory.listed, ["spaces/first", "spaces/second"]);
        assert!(directory.resolved.is_empty());
        assert_eq!(result["permanent_owners"], json!(["users/owner"]));
        assert_eq!(result["grants"].as_array().expect("grants").len(), 2);
        directory
            .members
            .get_mut("spaces/first")
            .expect("channel")
            .push("users/later".to_owned());
        let reopened = BridgeState::inspect(&scratch.root.join("state")).expect("reopen state");
        let saved = reopened.grants().expect("durable grants");
        assert_eq!(
            saved
                .iter()
                .map(|grant| grant.sender_id.as_str())
                .collect::<Vec<_>>(),
            ["users/first", "users/second"]
        );
        for grant in saved {
            assert_eq!(
                grant.expires_at_millis - grant.created_at_millis,
                43_200_000
            );
            assert_eq!(grant.note.as_deref(), Some("channel discussion"));
        }
        assert_eq!(
            reopened
                .subscribe_request()
                .expect("subscription")
                .allowed_senders()
                .iter()
                .map(|sender| sender.as_str())
                .collect::<Vec<_>>(),
            ["users/owner", "users/first", "users/second"]
        );
    }

    #[test]
    fn a_failed_member_listing_cannot_leave_partial_channel_grants() {
        let scratch = Scratch::new();
        let mut directory = Directory {
            members: BTreeMap::from([("spaces/first".to_owned(), vec!["users/first".to_owned()])]),
            ..Directory::default()
        };
        let error = grant_with_directory(
            &scratch.state,
            &[],
            true,
            Duration::from_secs(60),
            None,
            Some(&mut directory),
        )
        .expect_err("second channel fails");
        assert!(error.to_string().contains("incomplete_listing"));
        assert!(scratch
            .state
            .grants()
            .expect("no partial grants")
            .is_empty());
        assert!(!scratch.root.join("state/grants.json").exists());
    }

    #[test]
    fn a_name_is_resolved_before_only_its_provider_id_is_saved() {
        let scratch = Scratch::new();
        let mut directory = Directory::default();
        let result = grant_with_directory(
            &scratch.state,
            &["member@example.invalid".to_owned()],
            false,
            Duration::from_secs(60),
            None,
            Some(&mut directory),
        )
        .expect("resolved grant");
        assert_eq!(
            directory.resolved,
            [(
                "spaces/first".to_owned(),
                "member@example.invalid".to_owned()
            )]
        );
        assert_eq!(result["grants"][0]["sender_id"], "users/resolved");
        assert_eq!(
            scratch.state.grants().expect("grants")[0].sender_id,
            "users/resolved"
        );
    }
}
