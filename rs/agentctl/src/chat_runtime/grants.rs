//! Explicit, expiring sender authority, serialized independently from permanent owners.

use super::*;

/// Default maximum for a new sender grant: 30 days. Existing grants keep their expiry when the
/// configured maximum changes.
pub const DEFAULT_MAX_GRANT_SECONDS: u64 = 30 * 24 * 60 * 60;

const MAX_GRANTS: usize = 256;
const MAX_GRANT_DOCUMENT_BYTES: usize = 1 << 20;
const MAX_GRANT_NOTE_BYTES: usize = 2_000;

/// Parse a positive whole duration such as `30m`, `12h`, or `7d`. Supported suffixes are seconds
/// (`s`), minutes (`m`), hours (`h`), days (`d`), and weeks (`w`).
pub fn parse_grant_duration(value: &str) -> std::result::Result<Duration, String> {
    let invalid = || {
        "grant duration must be a positive integer followed by s, m, h, d, or w \
         (for example 30m, 12h, or 7d)"
            .to_owned()
    };
    let multiplier: u64 = match value.as_bytes().last() {
        Some(b's') => 1,
        Some(b'm') => 60,
        Some(b'h') => 60 * 60,
        Some(b'd') => 24 * 60 * 60,
        Some(b'w') => 7 * 24 * 60 * 60,
        _ => return Err(invalid()),
    };
    let digits = &value[..value.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let seconds = digits
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(multiplier))
        .ok_or_else(|| "grant duration is too large to represent in milliseconds".to_owned())?;
    if seconds == 0 {
        return Err(invalid());
    }
    seconds
        .checked_mul(1_000)
        .ok_or_else(|| "grant duration is too large to represent in milliseconds".to_owned())?;
    Ok(Duration::from_secs(seconds))
}

/// One explicit provider sender's time-limited authority to send new messages to the bridge.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SenderGrant {
    /// Stable provider sender ID, never a wildcard or a display name.
    pub sender_id: String,
    /// Unix milliseconds at which this grant was created or renewed.
    pub created_at_millis: u64,
    /// Unix milliseconds at which admission stops, including at this exact instant.
    pub expires_at_millis: u64,
    /// Optional operator explanation retained with the grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A durable marker prevents the service from logging the same expiry every batch or restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) expired_logged_at_millis: Option<u64>,
}

impl SenderGrant {
    pub(super) fn validate(&self) -> Result<()> {
        validate_grant_sender(&self.sender_id)?;
        validate_grant_note(self.note.as_deref())?;
        if self.expires_at_millis <= self.created_at_millis {
            return Err(ChatRuntimeError::invalid(
                "sender grant expiry must be later than its creation time",
            ));
        }
        if self
            .expired_logged_at_millis
            .is_some_and(|logged| logged < self.expires_at_millis)
        {
            return Err(ChatRuntimeError::invalid(
                "sender grant expiry cannot be logged before it expires",
            ));
        }
        Ok(())
    }

    pub(super) fn active_at(&self, now_millis: u64) -> bool {
        self.created_at_millis <= now_millis && now_millis < self.expires_at_millis
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantDocument {
    version: u32,
    grants: Vec<SenderGrant>,
}

impl GrantDocument {
    fn validate(&self) -> Result<()> {
        if self.version != STATE_VERSION {
            return Err(ChatRuntimeError::invalid(format!(
                "sender grants have unsupported version {}",
                self.version
            )));
        }
        if self.grants.len() > MAX_GRANTS {
            return Err(ChatRuntimeError::invalid(format!(
                "at most {MAX_GRANTS} sender grants may be retained; revoke expired grants to make room"
            )));
        }
        let mut senders = BTreeSet::new();
        for grant in &self.grants {
            grant.validate()?;
            if !senders.insert(grant.sender_id.as_str()) {
                return Err(ChatRuntimeError::invalid(format!(
                    "sender grants contain duplicate ID {:?}",
                    grant.sender_id
                )));
            }
        }
        Ok(())
    }
}

fn validate_grant_sender(sender: &str) -> Result<()> {
    if sender == "*" {
        return Err(ChatRuntimeError::invalid(
            "sender grants require explicit provider IDs; wildcard '*' is not allowed",
        ));
    }
    SenderId::new(sender.to_owned())
        .map(|_| ())
        .map_err(|error| ChatRuntimeError::invalid(error.to_string()))
}

fn validate_grant_note(note: Option<&str>) -> Result<()> {
    if note.is_some_and(|note| note.len() > MAX_GRANT_NOTE_BYTES) {
        return Err(ChatRuntimeError::invalid(format!(
            "sender grant note exceeds {MAX_GRANT_NOTE_BYTES} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn whole_duration_seconds(duration: Duration) -> Result<u64> {
    if duration.is_zero() || duration.subsec_nanos() != 0 {
        return Err(ChatRuntimeError::invalid(
            "grant duration must be a positive whole number of seconds",
        ));
    }
    duration.as_secs().checked_mul(1_000).ok_or_else(|| {
        ChatRuntimeError::invalid("grant duration is too large to represent in milliseconds")
    })?;
    Ok(duration.as_secs())
}

impl BridgeState {
    pub(super) fn read_grant_configuration_locked(&self) -> Result<ConfigurationEnvelope> {
        let envelope: ConfigurationEnvelope =
            read_document(&self.root.join("bridge.json"), 1 << 20)?;
        if envelope.version != STATE_VERSION {
            return Err(ChatRuntimeError::invalid(format!(
                "chat state has unsupported version {}",
                envelope.version
            )));
        }
        envelope.config.validate()?;
        Ok(envelope)
    }

    /// Create or renew each explicit sender grant atomically. Permanent owners are skipped;
    /// duplicate IDs produce one grant. The current on-disk maximum is checked under the lock.
    pub fn grant_senders(
        &self,
        ids: &[String],
        duration: Duration,
        note: Option<&str>,
    ) -> Result<Vec<SenderGrant>> {
        let duration_seconds = whole_duration_seconds(duration)?;
        validate_grant_note(note)?;
        for sender in ids {
            validate_grant_sender(sender)?;
        }
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        let envelope = self.read_grant_configuration_locked()?;
        if duration_seconds > envelope.config.max_grant_seconds {
            return Err(ChatRuntimeError::invalid(format!(
                "requested grant duration is {duration_seconds}s, exceeding max_grant_seconds={}s; \
                 choose a shorter --for duration or change the bridge maximum",
                envelope.config.max_grant_seconds
            )));
        }
        let created_at_millis = unix_millis();
        let expires_at_millis = created_at_millis
            .checked_add(duration_seconds * 1_000)
            .ok_or_else(|| ChatRuntimeError::invalid("sender grant expiry timestamp overflows"))?;
        let mut grants = self.read_grants_locked()?;
        let owners: BTreeSet<&str> = envelope
            .config
            .allowed_senders
            .iter()
            .map(String::as_str)
            .collect();
        let mut granted = Vec::new();
        for sender_id in ids.iter().collect::<BTreeSet<_>>() {
            if owners.contains(sender_id.as_str()) {
                continue;
            }
            let grant = SenderGrant {
                sender_id: sender_id.clone(),
                created_at_millis,
                expires_at_millis,
                note: note.map(str::to_owned),
                expired_logged_at_millis: None,
            };
            if let Some(existing) = grants
                .iter_mut()
                .find(|existing| existing.sender_id == *sender_id)
            {
                *existing = grant.clone();
            } else {
                grants.push(grant.clone());
            }
            granted.push(grant);
        }
        let mut allowed = owners;
        allowed.extend(
            grants
                .iter()
                .filter(|grant| grant.active_at(created_at_millis))
                .map(|grant| grant.sender_id.as_str()),
        );
        if allowed.len() > chat_subscription::MAX_ALLOWED_SENDERS {
            return Err(ChatRuntimeError::invalid(format!(
                "permanent owners and active grants exceed the maximum of {} allowed senders; \
                 revoke a grant before adding another sender",
                chat_subscription::MAX_ALLOWED_SENDERS
            )));
        }
        if !granted.is_empty() {
            self.write_grants_locked(grants)?;
        }
        Ok(granted)
    }

    /// Remove explicit grants atomically. A permanent owner's authority cannot be revoked with
    /// this command; unknown IDs are ignored. Returned IDs are the grants actually removed.
    pub fn revoke_senders(&self, ids: &[String]) -> Result<Vec<String>> {
        for sender in ids {
            validate_grant_sender(sender)?;
        }
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        let envelope = self.read_grant_configuration_locked()?;
        for sender in ids {
            if envelope.config.allowed_senders.contains(sender) {
                return Err(ChatRuntimeError::invalid(format!(
                    "sender {sender:?} is a permanent owner from allowed_senders and cannot be revoked"
                )));
            }
        }
        let ids: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
        let mut grants = self.read_grants_locked()?;
        let mut removed = Vec::new();
        grants.retain(|grant| {
            if ids.contains(grant.sender_id.as_str()) {
                removed.push(grant.sender_id.clone());
                false
            } else {
                true
            }
        });
        if !removed.is_empty() {
            self.write_grants_locked(grants)?;
        }
        Ok(removed)
    }

    /// Read all active and expired grants without creating or changing state.
    pub fn grants(&self) -> Result<Vec<SenderGrant>> {
        let _snapshot = self.lock_state_snapshot()?;
        self.read_grants_locked()
    }

    /// Change the maximum for future grants without reinitializing the bridge. Existing grants
    /// retain their expiry. Every grant operation rereads this value, including on this object.
    pub fn set_max_grant_duration(&self, duration: Duration) -> Result<u64> {
        let seconds = whole_duration_seconds(duration)?;
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        let mut envelope = self.read_grant_configuration_locked()?;
        envelope.config.max_grant_seconds = seconds;
        envelope.config.validate()?;
        write_document(&self.root.join("bridge.json"), &envelope)?;
        Ok(seconds)
    }

    /// Caller holds the shared or exclusive state lock. State without this optional document
    /// has no temporary grants.
    pub(super) fn read_grants_locked(&self) -> Result<Vec<SenderGrant>> {
        let document: GrantDocument =
            match read_document(&self.root.join("grants.json"), MAX_GRANT_DOCUMENT_BYTES) {
                Ok(document) => document,
                Err(ChatRuntimeError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(Vec::new());
                }
                Err(error) => return Err(error),
            };
        document.validate()?;
        Ok(document.grants)
    }

    fn write_grants_locked(&self, mut grants: Vec<SenderGrant>) -> Result<()> {
        grants.sort_by(|left, right| left.sender_id.cmp(&right.sender_id));
        let document = GrantDocument {
            version: STATE_VERSION,
            grants,
        };
        document.validate()?;
        if encoded_document_bytes(&document)? > MAX_GRANT_DOCUMENT_BYTES {
            return Err(ChatRuntimeError::invalid(format!(
                "sender grants document exceeds {MAX_GRANT_DOCUMENT_BYTES} bytes"
            )));
        }
        write_document(&self.root.join("grants.json"), &document)
    }

    /// Mark newly expired grants before returning them for one service log entry each. The
    /// durable marker survives subsequent batches and restarts and is reset when a grant renews.
    pub(crate) fn take_expired_grants(&self) -> Result<Vec<SenderGrant>> {
        let state_lock =
            agent::open_private_lock(&self.root.join(".state.lock"), "chat state lock")?;
        FileExt::lock_exclusive(&state_lock).map_err(ChatRuntimeError::Io)?;
        let now = unix_millis();
        let mut grants = self.read_grants_locked()?;
        let mut expired = Vec::new();
        for grant in &mut grants {
            if grant.expires_at_millis <= now && grant.expired_logged_at_millis.is_none() {
                grant.expired_logged_at_millis = Some(now);
                expired.push(grant.clone());
            }
        }
        if !expired.is_empty() {
            self.write_grants_locked(grants)?;
        }
        Ok(expired)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Barrier;

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        state: BridgeState,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_owners(vec!["users/owner".to_owned()])
        }

        fn with_owners(owners: Vec<String>) -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentctl-sender-grants-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let configuration = serde_json::from_value(json!({
                "subscription_plugin": "fixture",
                "channel_ids": ["spaces/example"],
                "allowed_senders": owners,
                "agent_name": "coordinator",
                "agent_label": "coordinator",
                "outbound_enabled": false,
                "ack_reaction": null
            }))
            .expect("legacy configuration without a grant maximum");
            let state = BridgeState::initialize(&root, configuration).expect("initialize state");
            Self { root, state }
        }

        fn seed(&self, grants: Vec<SenderGrant>) {
            let lock = agent::open_private_lock(&self.root.join(".state.lock"), "test state lock")
                .expect("open state lock");
            FileExt::lock_exclusive(&lock).expect("exclusive state lock");
            self.state.write_grants_locked(grants).expect("seed grants");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove state fixture");
        }
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn expired(sender: &str) -> SenderGrant {
        SenderGrant {
            sender_id: sender.to_owned(),
            created_at_millis: 1,
            expires_at_millis: 2,
            note: None,
            expired_logged_at_millis: None,
        }
    }

    #[test]
    fn grant_duration_parser_accepts_units_and_refuses_zero_fractional_or_overflow() {
        for (text, seconds) in [
            ("1s", 1),
            ("30m", 1_800),
            ("12h", 43_200),
            ("7d", 604_800),
            ("2w", 1_209_600),
        ] {
            assert_eq!(parse_grant_duration(text), Ok(Duration::from_secs(seconds)));
        }
        for text in [
            "",
            "0s",
            "12",
            "-1h",
            "+1h",
            "1.5h",
            "1h30m",
            "12H",
            " 1h",
            "1h ",
            "1\nh",
            "h",
            "18446744073709551615w",
            "18446744073709552s",
        ] {
            assert!(parse_grant_duration(text).is_err(), "accepted {text:?}");
        }
        assert_eq!(
            parse_grant_duration(&format!("{}s", u64::MAX / 1_000)),
            Ok(Duration::from_secs(u64::MAX / 1_000))
        );
    }

    #[test]
    fn absent_grants_are_read_only_and_legacy_config_defaults_to_thirty_days() {
        let fixture = Fixture::new();
        assert_eq!(fixture.state.config().max_grant_seconds, 30 * 24 * 60 * 60);
        assert_eq!(
            fixture.state.config().max_grant_seconds,
            DEFAULT_MAX_GRANT_SECONDS
        );
        assert!(fixture.state.grants().expect("legacy grants").is_empty());
        assert!(!fixture.root.join("grants.json").exists());
    }

    #[test]
    fn grant_activity_includes_creation_and_excludes_future_and_expiry() {
        let grant = SenderGrant {
            sender_id: "users/guest".to_owned(),
            created_at_millis: 100,
            expires_at_millis: 200,
            note: None,
            expired_logged_at_millis: None,
        };
        assert!(!grant.active_at(99));
        assert!(grant.active_at(100));
        assert!(grant.active_at(199));
        assert!(!grant.active_at(200));
    }

    #[test]
    fn grant_before_creation_is_not_logged_as_expired() {
        let fixture = Fixture::new();
        let now = unix_millis();
        fixture.seed(vec![SenderGrant {
            sender_id: "users/future".to_owned(),
            created_at_millis: now + 3_600_000,
            expires_at_millis: now + 7_200_000,
            note: None,
            expired_logged_at_millis: None,
        }]);
        let before = fs::read(fixture.root.join("grants.json")).expect("future grant bytes");
        assert!(fixture
            .state
            .take_expired_grants()
            .expect("future grant is not expired")
            .is_empty());
        assert_eq!(
            fs::read(fixture.root.join("grants.json")).expect("unmodified future grant"),
            before
        );
    }

    #[test]
    fn grant_persists_explicit_ids_times_and_notes_and_skips_permanent_owners() {
        let fixture = Fixture::new();
        let before = unix_millis();
        let created = fixture
            .state
            .grant_senders(
                &ids(&["users/second", "users/owner", "users/first", "users/second"]),
                Duration::from_secs(60),
                Some("temporary collaboration"),
            )
            .expect("grant explicit senders");
        let after = unix_millis();
        assert_eq!(created.len(), 2);
        assert_eq!(created[0].sender_id, "users/first");
        assert_eq!(created[1].sender_id, "users/second");
        for grant in &created {
            assert!((before..=after).contains(&grant.created_at_millis));
            assert_eq!(grant.expires_at_millis, grant.created_at_millis + 60_000);
            assert_eq!(grant.note.as_deref(), Some("temporary collaboration"));
            assert!(grant.active_at(grant.expires_at_millis - 1));
            assert!(!grant.active_at(grant.expires_at_millis));
        }
        let reopened = BridgeState::inspect(&fixture.root).expect("reopen state");
        assert_eq!(reopened.grants().expect("durable grants"), created);
        let metadata = fs::metadata(fixture.root.join("grants.json")).expect("grant metadata");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn grant_above_maximum_is_rejected_and_config_changes_affect_an_open_state() {
        let fixture = Fixture::new();
        let sender = ids(&["users/guest"]);
        let failure = fixture
            .state
            .grant_senders(
                &sender,
                Duration::from_secs(DEFAULT_MAX_GRANT_SECONDS + 1),
                None,
            )
            .expect_err("over maximum grant must fail");
        assert!(failure.to_string().contains("max_grant_seconds"));
        assert!(!fixture.root.join("grants.json").exists());
        assert_eq!(
            fixture
                .state
                .set_max_grant_duration(Duration::from_secs(60))
                .expect("change maximum"),
            60
        );
        assert!(fixture
            .state
            .grant_senders(&sender, Duration::from_secs(61), None)
            .is_err());
        let granted = fixture
            .state
            .grant_senders(&sender, Duration::from_secs(60), None)
            .expect("grant at the updated maximum");
        assert_eq!(granted.len(), 1);
        fixture
            .state
            .set_max_grant_duration(Duration::from_secs(1))
            .expect("lower maximum again");
        assert_eq!(fixture.state.grants().expect("unchanged grant"), granted);
        assert_eq!(
            BridgeState::inspect(&fixture.root)
                .expect("reopen configuration")
                .config()
                .max_grant_seconds,
            1
        );
        for invalid in [Duration::ZERO, Duration::from_millis(1), Duration::MAX] {
            assert!(fixture.state.set_max_grant_duration(invalid).is_err());
            assert!(fixture.state.grant_senders(&sender, invalid, None).is_err());
        }
    }

    #[test]
    fn grant_and_revoke_validation_is_atomic_and_owner_authority_is_permanent() {
        let fixture = Fixture::new();
        let sender = ids(&["users/guest"]);
        assert!(fixture
            .state
            .grant_senders(&ids(&["users/guest", "*"]), Duration::from_secs(60), None)
            .is_err());
        assert!(!fixture.root.join("grants.json").exists());
        assert!(fixture
            .state
            .grant_senders(&sender, Duration::from_secs(60), Some(&"x".repeat(2_001)))
            .is_err());
        assert!(!fixture.root.join("grants.json").exists());
        fixture
            .state
            .grant_senders(&sender, Duration::from_secs(60), None)
            .expect("grant guest");
        let before = fs::read(fixture.root.join("grants.json")).expect("before rejected revoke");
        let failure = fixture
            .state
            .revoke_senders(&ids(&["users/guest", "users/owner"]))
            .expect_err("owner cannot be revoked");
        assert!(failure.to_string().contains("permanent owner"));
        assert_eq!(
            fs::read(fixture.root.join("grants.json")).expect("after rejected revoke"),
            before
        );
        assert!(fixture
            .state
            .revoke_senders(&ids(&["users/unknown"]))
            .expect("unknown is a no-op")
            .is_empty());
        assert_eq!(
            fixture.state.revoke_senders(&sender).expect("revoke guest"),
            sender
        );
        assert!(BridgeState::inspect(&fixture.root)
            .expect("reopen state")
            .grants()
            .expect("revoked grants")
            .is_empty());
    }

    #[test]
    fn expiry_is_reported_once_across_reopens_and_renewal_resets_the_marker() {
        let fixture = Fixture::new();
        fixture.seed(vec![expired("users/guest")]);
        let expired = fixture.state.take_expired_grants().expect("new expiry");
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].sender_id, "users/guest");
        assert!(expired[0].expired_logged_at_millis.is_some());
        assert!(fixture
            .state
            .take_expired_grants()
            .expect("same process expiry")
            .is_empty());
        let reopened = BridgeState::inspect(&fixture.root).expect("reopen state");
        assert!(reopened
            .take_expired_grants()
            .expect("restart expiry")
            .is_empty());
        let renewed = reopened
            .grant_senders(
                &ids(&["users/guest"]),
                Duration::from_secs(60),
                Some("renewed"),
            )
            .expect("renew grant");
        assert_eq!(renewed.len(), 1);
        assert!(renewed[0].active_at(unix_millis()));
        assert!(renewed[0].expired_logged_at_millis.is_none());
        assert_eq!(fixture.state.grants().expect("renewed snapshot"), renewed);
    }

    #[test]
    fn active_union_and_retained_grant_limits_are_enforced_before_publish() {
        let fixture = Fixture::new();
        let guests = (0..chat_subscription::MAX_ALLOWED_SENDERS - 1)
            .map(|index| format!("users/guest-{index}"))
            .collect::<Vec<_>>();
        fixture
            .state
            .grant_senders(&guests, Duration::from_secs(60), None)
            .expect("fill active union");
        let before = fs::read(fixture.root.join("grants.json")).expect("filled grants");
        assert!(fixture
            .state
            .grant_senders(&ids(&["users/extra"]), Duration::from_secs(60), None)
            .is_err());
        assert_eq!(
            fs::read(fixture.root.join("grants.json")).expect("unchanged grants"),
            before
        );
        fixture.seed(
            (0..MAX_GRANTS)
                .map(|index| expired(&format!("users/expired-{index}")))
                .collect(),
        );
        let before = fs::read(fixture.root.join("grants.json")).expect("expired grants");
        assert!(fixture
            .state
            .grant_senders(&ids(&["users/new"]), Duration::from_secs(60), None)
            .is_err());
        assert_eq!(
            fs::read(fixture.root.join("grants.json")).expect("unchanged expired grants"),
            before
        );
    }

    #[test]
    fn malformed_durable_grants_are_refused_without_replacing_them() {
        let fixture = Fixture::new();
        let good = expired("users/guest");
        let mut wildcard = good.clone();
        wildcard.sender_id = "*".to_owned();
        let mut bad_expiry = good.clone();
        bad_expiry.expires_at_millis = bad_expiry.created_at_millis;
        let mut early_log = good.clone();
        early_log.expired_logged_at_millis = Some(1);
        for grants in [
            vec![good.clone(), good],
            vec![wildcard],
            vec![bad_expiry],
            vec![early_log],
        ] {
            write_document(
                &fixture.root.join("grants.json"),
                &GrantDocument {
                    version: STATE_VERSION,
                    grants,
                },
            )
            .expect("malformed grant fixture");
            let before = fs::read(fixture.root.join("grants.json")).expect("malformed bytes");
            assert!(fixture.state.grants().is_err());
            assert!(fixture
                .state
                .grant_senders(&ids(&["users/new"]), Duration::from_secs(60), None)
                .is_err());
            assert_eq!(
                fs::read(fixture.root.join("grants.json")).expect("unreplaced malformed bytes"),
                before
            );
        }
    }

    #[test]
    fn concurrent_grant_writers_preserve_each_explicit_sender() {
        let fixture = Fixture::new();
        let barrier = Arc::new(Barrier::new(8));
        let writers = (0..8)
            .map(|index| {
                let state = fixture.state.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    state
                        .grant_senders(
                            &[format!("users/concurrent-{index}")],
                            Duration::from_secs(60),
                            None,
                        )
                        .expect("concurrent grant")
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            assert_eq!(writer.join().expect("writer thread").len(), 1);
        }
        let grants = BridgeState::inspect(&fixture.root)
            .expect("reopen state")
            .grants()
            .expect("concurrent snapshot");
        assert_eq!(grants.len(), 8);
        assert_eq!(
            grants
                .into_iter()
                .map(|grant| grant.sender_id)
                .collect::<BTreeSet<_>>(),
            (0..8)
                .map(|index| format!("users/concurrent-{index}"))
                .collect()
        );
    }
}
