//! A Slack group DM is named by the people in it, not by its channel handle.
//! `#317 slack-group-dm-names`.
//!
//! Against the loopback Slack fake, in a workspace whose handles are numbers (where the handle name
//! said nothing) and in one whose handles are words, through the client's channel browser and
//! through the HTTP routes the page uses: browse, search by a person's name, add, and see the name
//! in the channel list the picker, the headers and speech take their names from.

mod slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, ts, Fake, FakeState, METHODS, SELF_USER, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::chat::ChatClient;
use vibe_talk::directory::DirectoryRequest;
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;

/// A group DM in a workspace whose handles are numbers.
const NUMERIC: &str = "G0000NUM01";
/// A group DM in a workspace whose handles are words.
const WORDS: &str = "G0000WRD01";
/// A group DM whose members Slack will not list.
const UNLISTED: &str = "G0000UNL01";
/// A group DM whose members nobody can name.
const NAMELESS: &str = "G0000NML01";
/// A group DM with one member who can be named and one who cannot.
const HALF: &str = "G0000HLF01";

fn user(id: &str, handle: &str, display: &str, real: &str) -> Value {
    json!({"id": id, "name": handle, "is_bot": false,
        "profile": {"display_name": display, "real_name": real}})
}

fn fixture() -> FakeState {
    let mut state = FakeState {
        memberships: vec![
            json!({"id": "C0000TEAM1", "name": "team"}),
            json!({"id": NUMERIC, "is_mpim": true, "name": "mpdm-100001--234567--345678-1"}),
            json!({"id": WORDS, "is_mpim": true, "name": "mpdm-owner--ada--alan-1"}),
            json!({"id": UNLISTED, "is_mpim": true, "name": "mpdm-owner--ada--grace-1"}),
            json!({"id": NAMELESS, "is_mpim": true, "name": "mpdm-100001--999001--999002-1"}),
            json!({"id": HALF, "is_mpim": true, "name": "mpdm-100001--234567--999001-1"}),
        ],
        ..FakeState::default()
    };
    // The owner is a member of every one of them, and so, in one, is the account this client
    // posts as: neither names a group DM.
    state.members.insert(
        NUMERIC.to_owned(),
        vec!["UOWNER001".into(), "UADA00001".into(), "UGRACE001".into()],
    );
    state.members.insert(
        WORDS.to_owned(),
        vec![
            "UADA00001".into(),
            SELF_USER.into(),
            "UOWNER001".into(),
            "UALAN0001".into(),
        ],
    );
    state.members.insert(
        NAMELESS.to_owned(),
        vec!["UOWNER001".into(), "UGHOST001".into(), "UGHOST002".into()],
    );
    state.members.insert(
        HALF.to_owned(),
        vec!["UOWNER001".into(), "UADA00001".into(), "UGHOST001".into()],
    );
    state.users.insert(
        "UOWNER001".into(),
        user("UOWNER001", "100001", "Owner", "The Owner"),
    );
    state.users.insert(
        "UADA00001".into(),
        user("UADA00001", "234567", "Ada Lovelace", "Augusta Ada King"),
    );
    // No display name: the real name is the one people see.
    state.users.insert(
        "UGRACE001".into(),
        user("UGRACE001", "345678", "", "Grace Hopper"),
    );
    state.users.insert(
        "UALAN0001".into(),
        user("UALAN0001", "alan", "Alan Turing", "Alan Turing"),
    );
    state.users.insert(
        SELF_USER.into(),
        json!({"id": SELF_USER, "name": "vibe-bot", "is_bot": true,
            "profile": {"display_name": "", "real_name": "vibe-bot"}}),
    );
    // So a group DM can be added: registration reads it first.
    state.history.insert(
        NUMERIC.to_owned(),
        vec![profiled(&ts(10, 0), "UADA00001", "Ada Lovelace", "lunch?")],
    );
    state
}

async fn browse(client: &dyn ChatClient, query: Option<&str>) -> Vec<(String, String)> {
    let mut request = DirectoryRequest {
        query: query.map(str::to_owned),
        cursor: None,
        limit: 4,
    };
    let mut names = Vec::new();
    loop {
        let page = client.discover_channels(&request).await.expect("a page");
        names.extend(
            page.entries
                .into_iter()
                .map(|entry| (entry.source, entry.name)),
        );
        request.cursor = page.next_cursor;
        if request.cursor.is_none() {
            return names;
        }
    }
}

fn named<'n>(names: &'n [(String, String)], id: &str) -> Option<&'n str> {
    names
        .iter()
        .find(|(source, _)| source == id)
        .map(|(_, name)| name.as_str())
}

fn assert_no_stray_methods(fake: &Fake) {
    fake.with(|state| {
        for (method, _) in &state.requests {
            assert!(
                METHODS.contains(&method.as_str()),
                "the client called {method}, outside the Web API subset"
            );
        }
    });
}

#[tokio::test]
async fn a_group_dm_is_named_by_its_other_members_display_names() {
    let fake = Fake::start(fixture(), 3).await;
    let client = fake.client();
    // The account this client posts as is learned as it always is, from `auth.test`.
    client.identity().await.expect("auth.test");
    let names = browse(&client, None).await;
    assert_eq!(names.len(), 6, "every membership once: {names:?}");

    // Numeric handles: the name the owner could not read is gone, and the people are there.
    assert_eq!(
        named(&names, NUMERIC),
        Some("Group: Ada Lovelace, Grace Hopper")
    );
    assert_ne!(
        named(&names, NUMERIC),
        Some("Group: 100001, 234567, 345678"),
        "the negative control: this is what the channel handle said"
    );
    // Word handles: the display names, neither the owner nor this client's own account among them.
    assert_eq!(
        named(&names, WORDS),
        Some("Group: Ada Lovelace, Alan Turing")
    );
    // Members Slack would not list: the handles, as before, because nothing better is known.
    assert_eq!(named(&names, UNLISTED), Some("Group: owner, ada, grace"));
    // Members nobody can name: the handles too, not two bare ids.
    assert_eq!(
        named(&names, NAMELESS),
        Some("Group: 100001, 999001, 999002")
    );
    // One known and one not: the name that is known, and the other by id, as a DM shows one.
    assert_eq!(named(&names, HALF), Some("Group: Ada Lovelace, UGHOST001"));
    // Ordinary channels are untouched.
    assert_eq!(named(&names, "C0000TEAM1"), Some("#team"));

    // One page of members per group DM, no more.
    for request in fake.requests("conversations.members") {
        assert_eq!(request.get("limit").map(String::as_str), Some("20"));
    }
    assert_no_stray_methods(&fake);
}

#[tokio::test]
async fn a_group_dm_is_found_by_a_members_name() {
    let fake = Fake::start(fixture(), 3).await;
    let client = fake.client();
    let found = browse(&client, Some("grace hop")).await;
    assert_eq!(
        found,
        [(
            NUMERIC.to_owned(),
            "Group: Ada Lovelace, Grace Hopper".to_owned()
        )],
        "searching a person's name finds the group DM they are in"
    );
    let found = browse(&client, Some("turing")).await;
    assert_eq!(
        found.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        [WORDS]
    );
}

#[tokio::test]
async fn a_group_dms_members_and_names_are_read_once() {
    let fake = Fake::start(fixture(), 3).await;
    let client = fake.client();
    let first = browse(&client, None).await;
    let members = fake.calls("conversations.members");
    let lookups = fake.calls("users.info");
    let mut asked: Vec<String> = fake
        .requests("conversations.members")
        .into_iter()
        .filter_map(|request| request.get("channel").cloned())
        .collect();
    asked.dedup();
    assert_eq!(
        asked,
        [NUMERIC, WORDS, UNLISTED, NAMELESS, HALF],
        "each group DM's members are read once, page after page where the server cut a page short"
    );
    // The one group DM of four members takes two pages from a server that answers three at a time.
    assert_eq!(members, 6);
    let again = browse(&client, None).await;
    assert_eq!(first, again);
    assert_eq!(
        fake.calls("conversations.members"),
        members,
        "members are remembered"
    );
    // Names found are remembered as authors' are; the ones that failed are remembered too, so a
    // missing name does not cost a call every time the browser is opened.
    assert_eq!(fake.calls("users.info"), lookups, "names are remembered");
}

#[tokio::test]
async fn without_users_read_a_group_dm_keeps_its_handles() {
    let fake = Fake::start(fixture(), 3).await;
    fake.with(|state| {
        state
            .fail
            .insert("users.info".to_owned(), "missing_scope".to_owned())
    });
    let names = browse(&fake.client(), None).await;
    assert_eq!(
        named(&names, NUMERIC),
        Some("Group: 100001, 234567, 345678")
    );
    assert_eq!(named(&names, WORDS), Some("Group: owner, ada, alan"));

    let fake = Fake::start(fixture(), 3).await;
    fake.with(|state| {
        state.fail.insert(
            "conversations.members".to_owned(),
            "missing_scope".to_owned(),
        )
    });
    let names = browse(&fake.client(), None).await;
    assert_eq!(
        named(&names, WORDS),
        Some("Group: owner, ada, alan"),
        "a refused members list leaves the handles, and the browser still answers"
    );
}

fn config_text(api_base: &str) -> String {
    format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{api_base}"
token = "{TOKEN}"
owner_user_id = "UOWNER001"
channel_registration = true
channel_discovery = true
registered_channels_writable = true

[[channels]]
id = "C0000TEAM1"
label = "team"
writable = true
"#,
        read = vibe_talk::testing::READ_TOKEN,
    )
}

async fn deployment(fake: &Fake) -> AppState {
    let text = config_text(&fake.base);
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_group_dm_added_from_the_browser_is_listed_by_its_members() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake).await;
    let (status, page) = call(
        &state,
        "GET",
        "/api/v1/channel-directory?limit=10&q=Ada",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let entry = page["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["source"] == NUMERIC)
        .cloned()
        .unwrap_or_else(|| panic!("the group DM is found by a member's name: {page}"));
    assert_eq!(entry["name"], "Group: Ada Lovelace, Grace Hopper");

    // The page adds a browsed channel under the name it was listed by.
    let (status, added) = call(
        &state,
        "POST",
        "/api/v1/channels",
        Some(json!({"source": NUMERIC, "label": entry["name"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(
        added["channel"]["label"],
        "Group: Ada Lovelace, Grace Hopper"
    );
    let (status, config) = call(&state, "GET", "/api/v1/client-config", None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    let listed = config["channels"]
        .as_array()
        .expect("channels")
        .iter()
        .find(|channel| channel["id"] == NUMERIC)
        .cloned()
        .unwrap_or_else(|| panic!("the added group DM is in the channel list: {config}"));
    assert_eq!(listed["label"], "Group: Ada Lovelace, Grace Hopper");
}
