//! Outbound helper behaviour against the fake transport: thread replies, the ACK reaction,
//! refusal without an app identity, and idempotent recovery of uncertain posts.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use chat_subscription_slack::fake::{FakePostFailure, FakeTransport};
use chat_subscription_slack::ids::AppId;
use chat_subscription_slack::outbound::{handle_request, OutboundConfig};
use chat_subscription_slack::transport::{
    EventStream, ListQuery, ListedMessage, PostMessage, SlackTransport, SubscribeSpec, TextFormat,
    TransportError,
};
use serde_json::{json, Value};

const CHANNEL: &str = "C0C91RW5FAR";
const APP: &str = "A0TESTAPP01";
const ID: &str = "0b7e4f6a-1c2d-4e5f-8a9b-0c1d2e3f4a5b";
const ID2: &str = "1b7e4f6a-1c2d-4e5f-9a9b-0c1d2e3f4a5b";

fn ledger(name: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("slack-outbound-ledgers")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    root
}

fn config(ledger: &Path, app: Option<&str>) -> OutboundConfig {
    let mut arguments = vec![
        "--cli=/opt/slack/bin/slack-cli".to_owned(),
        format!("--channel={CHANNEL}"),
        format!("--ledger={}", ledger.display()),
        "--max-chunk-bytes=1000".to_owned(),
    ];
    if let Some(app) = app {
        arguments.push(format!("--app-id={app}"));
    }
    OutboundConfig::parse(&arguments).expect("valid helper arguments")
}

fn run(config: &OutboundConfig, fake: &FakeTransport, request: &Value, now: u64) -> Value {
    let mut transport = || Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>);
    run_factory(config, request, now, &mut transport)
}

fn run_factory(
    config: &OutboundConfig,
    request: &Value,
    now: u64,
    transport: &mut dyn FnMut() -> Result<Box<dyn SlackTransport>, String>,
) -> Value {
    let clock = move || now;
    let mut line = serde_json::to_vec(request).unwrap();
    line.push(b'\n');
    let response = handle_request(&line, Ok(config), transport, &clock);
    assert_eq!(response.last(), Some(&b'\n'), "one response line");
    assert_eq!(response.iter().filter(|byte| **byte == b'\n').count(), 1);
    serde_json::from_slice(&response).expect("response is JSON")
}

fn run_transport(
    config: &OutboundConfig,
    request: &Value,
    transport: impl SlackTransport + 'static,
) -> Value {
    let mut transport = Some(Box::new(transport) as Box<dyn SlackTransport>);
    run_factory(config, request, 1_000, &mut || {
        Ok(transport.take().expect("one transport"))
    })
}

fn rejection_journal(ledger: &Path, id: &str) -> PathBuf {
    ledger.join("rejections").join(format!("{id}.json"))
}

fn assert_no_post_intent(ledger: &Path, id: &str) {
    assert!(
        !ledger.join(format!("{id}.json")).exists(),
        "a rejection never creates a posting intent"
    );
}

struct HookedReaction {
    fake: FakeTransport,
    after_human: Option<Box<dyn FnMut() + Send>>,
    before_reaction: Option<Box<dyn FnMut() + Send>>,
    lose_response: bool,
}

impl HookedReaction {
    fn new(fake: &FakeTransport) -> Self {
        Self {
            fake: fake.clone(),
            after_human: None,
            before_reaction: None,
            lose_response: false,
        }
    }
}

impl SlackTransport for HookedReaction {
    fn open_stream(
        &mut self,
        spec: &SubscribeSpec,
    ) -> Result<Box<dyn EventStream>, TransportError> {
        self.fake.open_stream(spec)
    }

    fn post_message(&mut self, post: &PostMessage<'_>) -> Result<String, TransportError> {
        self.fake.post_message(post)
    }

    fn list_messages(
        &mut self,
        query: &ListQuery<'_>,
    ) -> Result<Vec<ListedMessage>, TransportError> {
        self.fake.list_messages(query)
    }

    fn sender_is_human(&mut self, app_id: &AppId, sender_id: &str) -> Result<bool, TransportError> {
        let human = self.fake.sender_is_human(app_id, sender_id)?;
        if human {
            if let Some(hook) = self.after_human.as_mut() {
                hook();
            }
        }
        Ok(human)
    }

    fn add_reaction(
        &mut self,
        app_id: &AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError> {
        if let Some(hook) = self.before_reaction.as_mut() {
            hook();
        }
        let already_present = self
            .fake
            .add_reaction(app_id, channel_id, message_ts, name)?;
        if self.lose_response {
            Err(TransportError::unknown(
                "transport_timeout",
                "reaction applied but response lost",
            ))
        } else {
            Ok(already_present)
        }
    }
}

fn send(id: &str, thread: Option<&str>, text: &str) -> Value {
    json!({
        "version": 1, "id": id, "action": "send", "channel_id": CHANNEL,
        "thread_id": thread.map(|ts| format!("{CHANNEL}/threads/{ts}")),
        "text": text,
    })
}

fn rejection(sender_id: &str) -> Value {
    json!({
        "version": 1, "id": ID, "action": "ensure_rejection", "channel_id": CHANNEL,
        "message_id": format!("{CHANNEL}/messages/1792000801.000100"),
        "sender_id": sender_id, "emoji": "❌",
    })
}

#[test]
fn reply_goes_into_the_request_thread_as_the_app() {
    let ledger = ledger("thread");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let response = run(
        &config,
        &fake,
        &send(ID, Some("1792000801.000100"), "**done**"),
        1_000,
    );
    assert_eq!(response["ok"], json!(true), "{response}");
    assert_eq!(response["id"], json!(ID));
    assert_eq!(response["action"], json!("send"));
    let message_id = response["receipt"]["message_id"].as_str().unwrap();
    assert!(message_id.starts_with("C0C91RW5FAR/messages/"));
    let state = fake.state();
    assert_eq!(state.posts.len(), 1);
    assert_eq!(state.posts[0].app_id, APP);
    assert_eq!(
        state.posts[0].thread_ts.as_deref(),
        Some("1792000801.000100")
    );
    assert_eq!(state.posts[0].format, TextFormat::Markdown);
    assert_eq!(
        message_id,
        format!("{CHANNEL}/messages/{}", state.posts[0].ts)
    );
}

#[test]
fn ack_reaction_maps_the_emoji_and_is_idempotent() {
    let ledger = ledger("reaction");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let request = json!({
        "version": 1, "id": ID, "action": "ensure_reaction", "channel_id": CHANNEL,
        "message_id": format!("{CHANNEL}/messages/1792000801.000100"), "emoji": "🤖",
    });
    let first = run(&config, &fake, &request, 1_000);
    assert_eq!(first["ok"], json!(true), "{first}");
    assert_eq!(
        first["receipt"]["reaction_id"],
        json!(format!(
            "{CHANNEL}/messages/1792000801.000100/reactions/robot_face"
        ))
    );
    assert_eq!(first["receipt"]["already_present"], json!(false));
    let second = run(&config, &fake, &request, 2_000);
    assert_eq!(second["receipt"]["already_present"], json!(true));
    let state = fake.state();
    assert_eq!(state.reactions.len(), 1);
    assert_eq!(state.reactions[0].app_id, APP);
    assert_eq!(state.reactions[0].name, "robot_face");
    assert!(
        state.humanity_queries.is_empty(),
        "normal ACKs do not change protocol or need an author lookup"
    );
}

#[test]
fn rejection_requires_a_confirmed_human_and_is_idempotent() {
    let ledger = ledger("human-rejection");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), true);
    let request = rejection("U0GUEST001");
    let first = run(&config, &fake, &request, 1_000);
    assert_eq!(first["ok"], json!(true), "{first}");
    assert_eq!(first["id"], request["id"]);
    assert_eq!(first["action"], json!("ensure_rejection"));
    assert_eq!(
        first["receipt"],
        json!({
            "reaction_id": format!("{CHANNEL}/messages/1792000801.000100/reactions/x"),
            "already_present": false,
        })
    );
    let second = run(&config, &fake, &request, 2_000);
    assert_eq!(second["ok"], json!(true), "{second}");
    assert_eq!(second["receipt"]["already_present"], json!(true));
    let state = fake.state();
    assert_eq!(state.reactions.len(), 1);
    assert_eq!(state.reactions[0].app_id, APP);
    assert_eq!(state.reactions[0].name, "x");
    assert_eq!(
        state.humanity_queries,
        [(APP.to_owned(), "U0GUEST001".to_owned())],
        "completed receipt replay performs no fresh lookup or reaction"
    );
    assert_no_post_intent(&ledger, ID);
    let record: Value =
        serde_json::from_slice(&fs::read(rejection_journal(&ledger, ID)).unwrap()).unwrap();
    assert_eq!(record["name"], "x");
    assert_eq!(record["complete"], true);
}

#[test]
fn bot_and_unknown_accounts_never_receive_rejection_reactions() {
    let ledger = ledger("nonhuman-rejection");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.state()
        .sender_humanity
        .insert("U0BRIDGEBOT".to_owned(), false);
    // Both requests deliberately lack bot_id and app_id message annotations. A bot account
    // whose ID starts with U must still be refused by the authoritative account lookup.
    for sender in ["U0BRIDGEBOT", "U0UNKNOWN01"] {
        let response = run(&config, &fake, &rejection(sender), 1_000);
        assert_eq!(response["ok"], json!(false), "{response}");
        assert_eq!(response["error"]["code"], json!("author_not_human"));
        assert_eq!(response["error"]["outcome"], json!("not_applied"));
        assert_eq!(response["error"]["retryable"], json!(false));
        assert!(response.get("receipt").is_none());
    }
    assert_eq!(fake.state().humanity_queries.len(), 2);
    assert!(fake.state().reactions.is_empty());
    assert!(fake.state().posts.is_empty());
    assert_no_post_intent(&ledger, ID);
    assert!(
        !rejection_journal(&ledger, ID).exists(),
        "fresh nonhuman refusals create no possible-effect intent"
    );
}

#[test]
fn a_failed_human_lookup_never_falls_through_to_a_reaction() {
    let ledger = ledger("failed-author-lookup");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), true);
    fake.state()
        .humanity_failures
        .push_back(TransportError::not_applied(
            "ratelimited",
            "try later",
            true,
        ));
    let response = run(&config, &fake, &rejection("U0GUEST001"), 1_000);
    assert_eq!(response["ok"], json!(false), "{response}");
    assert_eq!(response["error"]["code"], json!("ratelimited"));
    assert_eq!(response["error"]["outcome"], json!("not_applied"));
    assert!(fake.state().reactions.is_empty());
    assert!(fake.state().posts.is_empty());
    assert_no_post_intent(&ledger, ID);
    assert!(!rejection_journal(&ledger, ID).exists());
}

#[test]
fn completed_rejection_replay_keeps_the_first_emoji_without_new_slack_calls() {
    let ledger = ledger("completed-first-choice");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), true);
    let mut request = rejection("U0GUEST001");
    let first = run(&config, &fake, &request, 1_000);
    assert_eq!(first["ok"], true, "{first}");
    let original = fs::read(rejection_journal(&ledger, ID)).unwrap();
    // A completed operation replays its historical receipt, not cached permission to write.
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), false);
    for changed_emoji in [":no_entry_sign:", "🚫"] {
        request["emoji"] = json!(changed_emoji);
        let replay = run_factory(&config, &request, 2_000, &mut || {
            panic!("a completed rejection replay must not construct a Slack client")
        });
        assert_eq!(replay["ok"], true, "{replay}");
        assert_eq!(
            replay["receipt"]["reaction_id"],
            first["receipt"]["reaction_id"]
        );
        assert_eq!(replay["receipt"]["already_present"], true);
        assert_eq!(fs::read(rejection_journal(&ledger, ID)).unwrap(), original);
    }
    let state = fake.state();
    assert_eq!(state.reactions.len(), 1);
    assert_eq!(state.reactions[0].name, "x");
    assert_eq!(state.reactions[0].app_id, APP);
    assert_eq!(state.reactions[0].channel_id, CHANNEL);
    assert_eq!(state.reactions[0].message_ts, "1792000801.000100");
    assert_eq!(state.humanity_queries.len(), 1);
    assert!(state.posts.is_empty());
    assert_no_post_intent(&ledger, ID);
}

#[test]
fn uncertain_rejection_replay_keeps_the_first_emoji_and_rechecks_the_author() {
    let ledger = ledger("uncertain-first-choice");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), true);
    let record_path = rejection_journal(&ledger, ID);
    let check_path = record_path.clone();
    let mut transport = HookedReaction::new(&fake);
    transport.lose_response = true;
    transport.before_reaction = Some(Box::new(move || {
        let record: Value = serde_json::from_slice(
            &fs::read(&check_path).expect("first choice is durable before a possible reaction"),
        )
        .unwrap();
        assert_eq!(
            record,
            json!({
                "schema": "agentctl.slack-rejection.v1", "id": ID, "app_id": APP,
                "channel_id": CHANNEL, "message_ts": "1792000801.000100",
                "sender_id": "U0GUEST001", "name": "x", "complete": false,
            })
        );
        let metadata = fs::metadata(&check_path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(
            fs::metadata(check_path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }));
    let mut request = rejection("U0GUEST001");
    let lost = run_transport(&config, &request, transport);
    assert_eq!(lost["ok"], false, "{lost}");
    assert_eq!(lost["error"]["outcome"], "unknown");
    assert_eq!(
        fake.state().reactions.len(),
        1,
        "Slack applied the response that was lost"
    );
    let pending: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(pending["complete"], false);
    // This is a new transport and handle_request invocation, with only disk carrying the intent.
    request["emoji"] = json!("no_entry_sign");
    let retry = run(&config, &fake, &request, 2_000);
    assert_eq!(retry["ok"], true, "{retry}");
    assert_eq!(retry["receipt"]["already_present"], true);
    assert_eq!(
        retry["receipt"]["reaction_id"],
        format!("{CHANNEL}/messages/1792000801.000100/reactions/x")
    );
    let state = fake.state();
    assert_eq!(
        state.reactions.len(),
        1,
        "uncertain retries must not add a different Slack marker"
    );
    assert_eq!(state.reactions[0].name, "x");
    assert_eq!(
        state.humanity_queries.len(),
        2,
        "each possible write rechecks the author"
    );
    assert!(state.posts.is_empty());
    let complete: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(complete["complete"], true);
    assert_eq!(complete["name"], "x");
    assert_no_post_intent(&ledger, ID);
}

#[test]
fn a_refused_retry_preserves_the_uncertainty_of_an_earlier_rejection() {
    let ledger = ledger("uncertain-author-refusal");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), true);
    let mut transport = HookedReaction::new(&fake);
    transport.lose_response = true;
    let mut request = rejection("U0GUEST001");
    let first = run_transport(&config, &request, transport);
    assert_eq!(first["error"]["outcome"], "unknown");
    let pending = fs::read(rejection_journal(&ledger, ID)).unwrap();
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), false);
    // Even an unsupported replacement emoji cannot discard the prior operation's evidence.
    request["emoji"] = json!("🚫");
    let refused = run(&config, &fake, &request, 2_000);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "author_not_human");
    assert_eq!(refused["error"]["outcome"], "unknown");
    assert_eq!(refused["error"]["retryable"], true);
    assert_eq!(fake.state().humanity_queries.len(), 2);
    assert_eq!(
        fake.state().reactions.len(),
        1,
        "a failed new account check cannot write"
    );
    assert_eq!(fake.state().reactions[0].name, "x");
    assert_eq!(fs::read(rejection_journal(&ledger, ID)).unwrap(), pending);
    let unavailable = run_factory(&config, &request, 3_000, &mut || {
        Err("client unavailable".to_owned())
    });
    assert_eq!(unavailable["error"]["code"], "helper_misconfigured");
    assert_eq!(unavailable["error"]["outcome"], "unknown");
    assert!(fake.state().posts.is_empty());
    assert_no_post_intent(&ledger, ID);
}

#[test]
fn reused_rejection_uuid_cannot_change_its_app_channel_message_or_sender() {
    let ledger = ledger("rejection-identity-conflicts");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.state()
        .sender_humanity
        .insert("U0GUEST001".to_owned(), true);
    let request = rejection("U0GUEST001");
    let first = run(&config, &fake, &request, 1_000);
    assert_eq!(first["ok"], true, "{first}");
    let original = fs::read(rejection_journal(&ledger, ID)).unwrap();
    let mut other_app = config.clone();
    other_app.app_id = Some(AppId::new("A0OTHER001").unwrap());
    let mut channel_config = config.clone();
    channel_config.channels.push("C0OTHER001".to_owned());
    let mut other_channel = request.clone();
    other_channel["channel_id"] = json!("C0OTHER001");
    other_channel["message_id"] = json!("C0OTHER001/messages/1792000801.000100");
    let mut other_message = request.clone();
    other_message["message_id"] = json!(format!("{CHANNEL}/messages/1792000802.000100"));
    let mut other_sender = request.clone();
    other_sender["sender_id"] = json!("U0OTHER001");
    for (config, request) in [
        (&other_app, request),
        (&channel_config, other_channel),
        (&config, other_message),
        (&config, other_sender),
    ] {
        let refused = run_factory(config, &request, 2_000, &mut || {
            panic!("a rejection identity conflict must contact nothing")
        });
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(refused["error"]["code"], "rejection_conflict");
        assert_eq!(refused["error"]["outcome"], "not_applied");
        assert_eq!(refused["error"]["retryable"], false);
        assert_eq!(fs::read(rejection_journal(&ledger, ID)).unwrap(), original);
    }
    assert_eq!(fake.state().humanity_queries.len(), 1);
    assert_eq!(fake.state().reactions.len(), 1);
    assert!(fake.state().posts.is_empty());
}

#[test]
fn corrupt_oversized_or_unsafe_rejection_journals_contact_nothing() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    for case in [
        "corrupt",
        "oversized",
        "symlink",
        "hardlink",
        "permissions",
        "fifo",
        "duplicate-key",
    ] {
        let ledger = ledger(&format!("rejection-journal-{case}"));
        let config = config(&ledger, Some(APP));
        let fake = FakeTransport::new();
        fake.state()
            .sender_humanity
            .insert("U0GUEST001".to_owned(), true);
        let request = rejection("U0GUEST001");
        let first = run(&config, &fake, &request, 1_000);
        assert_eq!(first["ok"], true, "{first}");
        let record = rejection_journal(&ledger, ID);
        match case {
            "corrupt" => fs::write(&record, b"{").unwrap(),
            "oversized" => {
                let mut valid = fs::read(&record).unwrap();
                valid.resize(4097, b' ');
                serde_json::from_slice::<Value>(&valid)
                    .expect("only the size guard rejects this valid journal");
                fs::write(&record, valid).unwrap();
            }
            "symlink" => {
                let target = record.with_extension("saved");
                fs::rename(&record, &target).unwrap();
                std::os::unix::fs::symlink(target, &record).unwrap();
            }
            "hardlink" => fs::hard_link(&record, record.with_extension("linked")).unwrap(),
            "permissions" => {
                fs::set_permissions(&record, fs::Permissions::from_mode(0o644)).unwrap()
            }
            "fifo" => {
                fs::remove_file(&record).unwrap();
                let name = CString::new(record.as_os_str().as_bytes()).unwrap();
                // SAFETY: name is a NUL-terminated fixture path; the FIFO is private and never read.
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
            "duplicate-key" => {
                let original = fs::read_to_string(&record).unwrap();
                fs::write(&record, original.replacen('{', "{\"complete\":false,", 1)).unwrap();
            }
            _ => unreachable!(),
        }
        let refused = run_factory(&config, &request, 2_000, &mut || {
            panic!("{case}: an unreadable rejection journal must not construct a client")
        });
        assert_eq!(refused["ok"], false, "{case}: {refused}");
        assert_eq!(refused["error"]["code"], "rejection_ledger_unreadable");
        assert_eq!(refused["error"]["outcome"], "unknown");
        assert_eq!(refused["error"]["retryable"], true);
        assert_eq!(fake.state().humanity_queries.len(), 1);
        assert_eq!(fake.state().reactions.len(), 1);
        assert!(fake.state().posts.is_empty());
        assert_no_post_intent(&ledger, ID);
    }
}

#[test]
fn rejection_storage_requires_an_existing_parent_and_refuses_symlink_ancestry() {
    let root = ledger("rejection-storage-anchor");
    let fake = FakeTransport::new();
    let request = rejection("U0GUEST001");
    let missing_parent = root.join("not-provisioned").join("ledger");
    let missing_config = config(&missing_parent, Some(APP));
    let refused = run_factory(&missing_config, &request, 1_000, &mut || {
        panic!("missing storage anchors cannot start a client")
    });
    assert_eq!(refused["error"]["code"], "rejection_ledger_unreadable");
    assert_eq!(refused["error"]["outcome"], "unknown");
    assert!(
        !root.exists(),
        "a rejected storage anchor creates no missing ancestors"
    );

    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(root.join("real")).unwrap();
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
    let symlink_ledger = root.join("alias/ledger");
    let symlink_config = config(&symlink_ledger, Some(APP));
    let refused = run_factory(&symlink_config, &request, 1_000, &mut || {
        panic!("a symlink in the ledger ancestry cannot start a client")
    });
    assert_eq!(refused["error"]["code"], "rejection_ledger_unreadable");
    assert_eq!(refused["error"]["outcome"], "unknown");
    assert!(!root.join("real/ledger").exists());
    assert!(fake.state().humanity_queries.is_empty());
    assert!(fake.state().reactions.is_empty());
    assert!(fake.state().posts.is_empty());
}

#[test]
fn rejection_persistence_failure_prevents_a_write_or_a_false_success_receipt() {
    for before_effect in [true, false] {
        let ledger = ledger(if before_effect {
            "rejection-bind-failure"
        } else {
            "rejection-complete-failure"
        });
        let config = config(&ledger, Some(APP));
        let fake = FakeTransport::new();
        fake.state()
            .sender_humanity
            .insert("U0GUEST001".to_owned(), true);
        let record = rejection_journal(&ledger, ID);
        let blocked = record.clone();
        let mut transport = HookedReaction::new(&fake);
        if before_effect {
            transport.after_human = Some(Box::new(move || {
                fs::create_dir(&blocked).unwrap();
            }));
        } else {
            transport.before_reaction = Some(Box::new(move || {
                fs::rename(&blocked, blocked.with_extension("intent")).unwrap();
                fs::create_dir(&blocked).unwrap();
            }));
        }
        let mut request = rejection("U0GUEST001");
        let failed = run_transport(&config, &request, transport);
        assert_eq!(failed["ok"], false, "{failed}");
        assert_eq!(failed["error"]["code"], "rejection_ledger_unwritable");
        assert_eq!(failed["error"]["outcome"], "unknown");
        assert!(failed.get("receipt").is_none());
        assert_eq!(fake.state().reactions.len(), usize::from(!before_effect));
        assert_eq!(fake.state().humanity_queries.len(), 1);
        assert!(fake.state().posts.is_empty());
        assert_no_post_intent(&ledger, ID);
        if !before_effect {
            fs::remove_dir(&record).unwrap();
            fs::rename(record.with_extension("intent"), &record).unwrap();
            request["emoji"] = json!("no_entry_sign");
            let retry = run(&config, &fake, &request, 2_000);
            assert_eq!(retry["ok"], true, "{retry}");
            assert_eq!(retry["receipt"]["already_present"], true);
            assert_eq!(fake.state().reactions.len(), 1);
            assert_eq!(fake.state().reactions[0].name, "x");
            assert_eq!(fake.state().humanity_queries.len(), 2);
        }
    }
}

#[test]
fn rejection_validates_sender_shape_channel_and_exact_fields_before_transport() {
    let config = config(&ledger("rejection-local-refusal"), Some(APP));
    let fake = FakeTransport::new();
    let mut other_channel = rejection("U0GUEST001");
    other_channel["channel_id"] = json!("C0OTHER0001");
    other_channel["message_id"] = json!("C0OTHER0001/messages/1792000801.000100");
    let mut extra = rejection("U0GUEST001");
    extra["text"] = json!("must not be accepted");
    let mut missing = rejection("U0GUEST001");
    missing.as_object_mut().unwrap().remove("sender_id");
    for (request, code) in [
        (rejection("member@example.invalid"), "request_invalid"),
        (other_channel, "channel_not_allowed"),
        (extra, "request_invalid"),
        (missing, "request_invalid"),
    ] {
        let mut built = false;
        let mut transport = || {
            built = true;
            Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>)
        };
        let response: Value = serde_json::from_slice(&handle_request(
            &serde_json::to_vec(&request).unwrap(),
            Ok(&config),
            &mut transport,
            &|| 1_000,
        ))
        .unwrap();
        assert_eq!(response["ok"], json!(false), "{response}");
        assert_eq!(response["error"]["code"], json!(code));
        assert_eq!(response["id"], request["id"]);
        assert_eq!(response["action"], request["action"]);
        assert!(!built, "locally refused requests cannot start any client");
    }
    assert!(fake.state().reactions.is_empty());
    assert!(fake.state().humanity_queries.is_empty());
}

#[test]
fn without_an_app_id_nothing_is_posted() {
    let ledger = ledger("no-app");
    let config = config(&ledger, None);
    let fake = FakeTransport::new();
    let mut built = 0;
    let mut transport = || {
        built += 1;
        Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>)
    };
    let line = serde_json::to_vec(&send(ID, Some("1792000801.000100"), "hello")).unwrap();
    let response: Value =
        serde_json::from_slice(&handle_request(&line, Ok(&config), &mut transport, &|| 1)).unwrap();
    assert_eq!(response["ok"], json!(false));
    assert_eq!(response["error"]["code"], json!("app_identity_required"));
    assert_eq!(response["error"]["outcome"], json!("not_applied"));
    assert_eq!(built, 0, "the transport is never built without an app id");
    assert!(fake.state().posts.is_empty());

    // A user ID is not accepted as the posting identity.
    assert!(OutboundConfig::parse(&[
        "--cli=/opt/slack/bin/slack-cli".to_owned(),
        format!("--channel={CHANNEL}"),
        format!("--ledger={}", ledger.display()),
        "--app-id=U0OWNER0001".to_owned(),
    ])
    .is_err());
}

#[test]
fn a_retried_operation_returns_the_recorded_message() {
    let ledger = ledger("retry");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let request = send(ID, Some("1792000801.000100"), "once");
    let first = run(&config, &fake, &request, 1_000);
    let second = run(&config, &fake, &request, 5_000);
    assert_eq!(first["receipt"], second["receipt"]);
    assert_eq!(fake.state().posts.len(), 1);

    // The same UUID for a different message is refused.
    let conflict = run(
        &config,
        &fake,
        &send(ID, Some("1792000801.000100"), "different"),
        6_000,
    );
    assert_eq!(conflict["error"]["code"], json!("operation_conflict"));
}

#[test]
fn a_lost_response_is_reconciled_without_a_second_post() {
    let ledger = ledger("lost-response");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::PostedButLost);
    let request = send(ID, Some("1792000801.000100"), "[agent] the answer is 42");
    let first = run(&config, &fake, &request, 1_792_000_900_000);
    assert_eq!(first["error"]["outcome"], json!("unknown"));
    assert_eq!(first["error"]["retryable"], json!(true));
    let second = run(&config, &fake, &request, 1_792_000_960_000);
    assert_eq!(second["ok"], json!(true), "{second}");
    let state = fake.state();
    assert_eq!(
        state.posts.len(),
        1,
        "the earlier post was adopted, not repeated"
    );
    assert_eq!(
        second["receipt"]["message_id"],
        json!(format!("{CHANNEL}/messages/{}", state.posts[0].ts))
    );
}

#[test]
fn a_lost_request_is_reposted_only_after_the_uncertainty_window() {
    let ledger = ledger("lost-request");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::LostBeforePosting);
    let request = send(ID, Some("1792000801.000100"), "[agent] retry me");
    let start = 1_792_000_900_000;
    assert_eq!(
        run(&config, &fake, &request, start)["error"]["outcome"],
        json!("unknown")
    );
    let early = run(&config, &fake, &request, start + 30_000);
    assert_eq!(early["error"]["code"], json!("outcome_unresolved"));
    assert!(fake.state().posts.is_empty());
    let late = run(&config, &fake, &request, start + 121_000);
    assert_eq!(late["ok"], json!(true), "{late}");
    assert_eq!(fake.state().posts.len(), 1);
}

#[test]
fn slack_refusals_pass_through_as_not_applied() {
    let ledger = ledger("refused");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::Refuse(TransportError::not_applied(
        "not_in_channel",
        "the app is not a member of the channel",
        true,
    )));
    let request = send(ID, Some("1792000801.000100"), "hello");
    let refused = run(&config, &fake, &request, 1_000);
    assert_eq!(refused["error"]["code"], json!("not_in_channel"));
    assert_eq!(refused["error"]["outcome"], json!("not_applied"));
    // After the app is invited, the same UUID posts normally and immediately.
    assert_eq!(run(&config, &fake, &request, 2_000)["ok"], json!(true));
    assert_eq!(fake.state().posts.len(), 1);
}

#[test]
fn local_checks_refuse_before_contacting_slack() {
    let ledger = ledger("local");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let other_channel = json!({
        "version": 1, "id": ID, "action": "send", "channel_id": "C0OTHER0001",
        "thread_id": null, "text": "x",
    });
    assert_eq!(
        run(&config, &fake, &other_channel, 1)["error"]["code"],
        json!("channel_not_allowed")
    );
    let crossed_thread = json!({
        "version": 1, "id": ID, "action": "send", "channel_id": CHANNEL,
        "thread_id": "C0OTHER0001/threads/1792000801.000100", "text": "x",
    });
    assert_eq!(
        run(&config, &fake, &crossed_thread, 1)["error"]["code"],
        json!("request_invalid")
    );
    let unmapped = json!({
        "version": 1, "id": ID2, "action": "ensure_reaction", "channel_id": CHANNEL,
        "message_id": format!("{CHANNEL}/messages/1792000801.000100"), "emoji": "🦀",
    });
    assert_eq!(
        run(&config, &fake, &unmapped, 1)["error"]["code"],
        json!("reaction_unsupported")
    );
    let garbage = handle_request(b"not json\n", Ok(&config), &mut || unreachable!(), &|| 1);
    let garbage: Value = serde_json::from_slice(&garbage).unwrap();
    assert_eq!(garbage["id"], Value::Null);
    assert!(fake.state().posts.is_empty() && fake.state().reactions.is_empty());
}

#[test]
fn long_bodies_are_posted_in_order_and_resume_after_a_partial_failure() {
    let body = (0..30)
        .map(|line| format!("line {line:02} {}\n", "x".repeat(80)))
        .collect::<String>();

    let ledger = ledger("chunks");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let first = run(&config, &fake, &send(ID, None, &body), 1_000);
    assert_eq!(first["ok"], json!(true), "{first}");
    let posts = fake.state().posts.clone();
    assert!(posts.len() >= 3, "a 2.6 KB body is split into 1 KB chunks");
    assert_eq!(
        posts
            .iter()
            .map(|post| post.text.as_str())
            .collect::<String>(),
        body
    );
    assert!(posts
        .iter()
        .all(|post| post.thread_ts.is_none() && post.app_id == APP));
    assert_eq!(
        first["receipt"]["message_id"],
        json!(format!("{CHANNEL}/messages/{}", posts[0].ts))
    );

    // Chunk two is refused; the retry of the same UUID posts only the remaining chunks.
    let ledger = self::ledger("chunks-partial");
    let config = self::config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let line = serde_json::to_vec(&send(ID2, None, &body)).unwrap();
    let mut refusing = Some(SecondRefusal {
        inner: fake.clone(),
        posts: 0,
    });
    let mut wrapped =
        || Ok(Box::new(refusing.take().expect("built once")) as Box<dyn SlackTransport>);
    let response: Value =
        serde_json::from_slice(&handle_request(&line, Ok(&config), &mut wrapped, &|| 1_000))
            .unwrap();
    assert_eq!(response["error"]["code"], json!("ratelimited"));
    assert_eq!(fake.state().posts.len(), 1);
    let mut plain = || Ok(Box::new(fake.clone()) as Box<dyn SlackTransport>);
    let response: Value =
        serde_json::from_slice(&handle_request(&line, Ok(&config), &mut plain, &|| 2_000)).unwrap();
    assert_eq!(response["ok"], json!(true), "{response}");
    let posts = fake.state().posts.clone();
    assert_eq!(
        posts
            .iter()
            .map(|post| post.text.as_str())
            .collect::<String>(),
        body
    );
}

/// A transport that lets one post through and refuses the next with `ratelimited`.
struct SecondRefusal {
    inner: FakeTransport,
    posts: usize,
}

impl SlackTransport for SecondRefusal {
    fn open_stream(
        &mut self,
        spec: &chat_subscription_slack::transport::SubscribeSpec,
    ) -> Result<Box<dyn chat_subscription_slack::transport::EventStream>, TransportError> {
        self.inner.open_stream(spec)
    }

    fn post_message(
        &mut self,
        post: &chat_subscription_slack::transport::PostMessage<'_>,
    ) -> Result<String, TransportError> {
        self.posts += 1;
        if self.posts == 2 {
            return Err(TransportError::not_applied(
                "ratelimited",
                "slow down",
                true,
            ));
        }
        self.inner.post_message(post)
    }

    fn add_reaction(
        &mut self,
        app_id: &chat_subscription_slack::ids::AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError> {
        self.inner
            .add_reaction(app_id, channel_id, message_ts, name)
    }

    fn list_messages(
        &mut self,
        query: &chat_subscription_slack::transport::ListQuery<'_>,
    ) -> Result<Vec<chat_subscription_slack::transport::ListedMessage>, TransportError> {
        self.inner.list_messages(query)
    }
}

/// A transport whose listing always fails, as for an app without history access.
struct FailingListing {
    inner: FakeTransport,
}

impl SlackTransport for FailingListing {
    fn open_stream(
        &mut self,
        spec: &chat_subscription_slack::transport::SubscribeSpec,
    ) -> Result<Box<dyn chat_subscription_slack::transport::EventStream>, TransportError> {
        self.inner.open_stream(spec)
    }

    fn post_message(
        &mut self,
        post: &chat_subscription_slack::transport::PostMessage<'_>,
    ) -> Result<String, TransportError> {
        self.inner.post_message(post)
    }

    fn add_reaction(
        &mut self,
        app_id: &chat_subscription_slack::ids::AppId,
        channel_id: &str,
        message_ts: &str,
        name: &str,
    ) -> Result<bool, TransportError> {
        self.inner
            .add_reaction(app_id, channel_id, message_ts, name)
    }

    fn list_messages(
        &mut self,
        _query: &chat_subscription_slack::transport::ListQuery<'_>,
    ) -> Result<Vec<chat_subscription_slack::transport::ListedMessage>, TransportError> {
        Err(TransportError::not_applied(
            "missing_scope",
            "no history scope",
            true,
        ))
    }
}

#[test]
fn a_failing_listing_still_reposts_after_the_window() {
    let ledger = ledger("failing-listing");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::LostBeforePosting);
    let line = serde_json::to_vec(&send(ID, Some("1792000801.000100"), "[agent] hi")).unwrap();
    let mut transport = || {
        Ok(Box::new(FailingListing {
            inner: fake.clone(),
        }) as Box<dyn SlackTransport>)
    };
    let start = 1_792_000_900_000;
    let mut at = |now: u64| -> Value {
        serde_json::from_slice(&handle_request(&line, Ok(&config), &mut transport, &|| now))
            .unwrap()
    };
    assert_eq!(at(start)["error"]["outcome"], json!("unknown"));
    assert_eq!(
        at(start + 60_000)["error"]["code"],
        json!("outcome_unresolved")
    );
    assert_eq!(at(start + 121_000)["ok"], json!(true));
    assert_eq!(fake.state().posts.len(), 1);
}

#[test]
fn converted_format_posts_converted_mrkdwn_and_env_overrides() {
    let ledger = ledger("converted-format");
    // Helper configured for the vendor markdown path; the request selects converted.
    let config = config(&ledger, Some(APP));
    assert_eq!(config.format, TextFormat::Markdown);
    let fake = FakeTransport::new();
    let mut request = send(
        ID,
        Some("1792000801.000100"),
        "# Report\n\n**Done** & <ok>\n\n- [x] shipped",
    );
    request["slack_format"] = json!("converted");
    let response = run(&config, &fake, &request, 1_000);
    assert_eq!(response["ok"], json!(true), "{response}");
    let posts = &fake.state().posts;
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].format, TextFormat::SlackMrkdwn);
    assert_eq!(
        posts[0].text,
        "*Report*\n\n*Done* &amp; &lt;ok&gt;\n\n• ☑ shipped"
    );
    // `#551 headings-real-headings`: the heading is also a header block, the text the fallback.
    let blocks: Value =
        serde_json::from_str(posts[0].blocks.as_deref().expect("blocks")).expect("blocks JSON");
    assert_eq!(
        blocks,
        json!([
            {"type": "header", "text": {"type": "plain_text", "text": "Report", "emoji": true}},
            {"type": "section", "text": {"type": "mrkdwn", "text": "*Done* &amp; &lt;ok&gt;\n\n• ☑ shipped"}}
        ])
    );

    // The environment override converts a plain helper configuration.
    let env_config = OutboundConfig::parse_with_environment(
        &[
            "--cli=/opt/slack/bin/slack-cli".to_owned(),
            format!("--channel={CHANNEL}"),
            format!("--ledger={}", ledger.display()),
        ],
        Some("converted"),
    )
    .expect("env override");
    assert_eq!(env_config.format, TextFormat::Converted);
    assert!(OutboundConfig::parse_with_environment(
        &[
            "--cli=/opt/slack/bin/slack-cli".to_owned(),
            format!("--channel={CHANNEL}"),
            format!("--ledger={}", ledger.display()),
        ],
        Some("bogus"),
    )
    .is_err());
}

#[test]
fn converted_replies_without_headings_and_markdown_replies_carry_no_blocks() {
    let ledger = ledger("converted-no-headings");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    let mut request = send(ID, Some("1792000801.000100"), "Plain **reply**.\n\n- one");
    request["slack_format"] = json!("converted");
    assert_eq!(run(&config, &fake, &request, 1_000)["ok"], json!(true));
    let mut markdown = send(ID2, Some("1792000801.000100"), "# A heading\n\nbody");
    markdown["slack_format"] = json!("markdown");
    assert_eq!(run(&config, &fake, &markdown, 1_000)["ok"], json!(true));
    let posts = &fake.state().posts;
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0].blocks, None);
    assert_eq!(posts[1].format, TextFormat::Markdown);
    assert_eq!(posts[1].blocks, None);
}

#[test]
fn refused_blocks_fall_back_to_the_text_alone_instead_of_losing_the_reply() {
    // `#551 headings-real-headings`: Slack may count a header's characters differently; the mrkdwn
    // text alone is always a valid post, so the chunk is said as text rather than lost.
    let ledger = ledger("converted-blocks-refused");
    let config = config(&ledger, Some(APP));
    let fake = FakeTransport::new();
    fake.push_post_failure(FakePostFailure::Refuse(TransportError::not_applied(
        "invalid_blocks",
        "invalid_blocks",
        false,
    )));
    let mut request = send(ID, Some("1792000801.000100"), "# Report\n\nbody");
    request["slack_format"] = json!("converted");
    let response = run(&config, &fake, &request, 1_000);
    assert_eq!(response["ok"], json!(true), "{response}");
    let posts = &fake.state().posts;
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].text, "*Report*\n\nbody");
    assert_eq!(posts[0].blocks, None);
}
