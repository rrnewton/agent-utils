//! A Slack mention is drawn and said as the person's name. `#318 slack-mention-names`.
//!
//! Against the loopback Slack fake: who each `<@U…>` names comes with every read, from the name
//! cache authors already use, a bounded lookup for people not yet seen and a legacy mention's own
//! label; an unknown id stays an id. Through the HTTP routes the page reads, the chip shows the
//! name, speech says it, and the body and what is posted keep `<@U…>`. A channel mention Slack
//! sends without its name is named from `conversations.info`, and a user group keeps its handle.

mod slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{plain, profiled, ts, Fake, FakeState, METHODS, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::chat::ChatClient;
use vibe_talk::model::{ChannelId, MentionName};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;

const TEAM: &str = "C0000TEAM1";
const OPS: &str = "C0000OPS01";

fn user(id: &str, display: &str, real: &str) -> Value {
    json!({"id": id, "name": id.to_lowercase(), "is_bot": false,
        "profile": {"display_name": display, "real_name": real}})
}

fn named(id: &str, name: &str) -> MentionName {
    MentionName {
        id: id.to_owned(),
        name: name.to_owned(),
    }
}

/// Ada writes, so her name is known from her own message. Grace never writes, so hers is looked
/// up. Nobody can name `U0GHOST01`. Alan is mentioned the old way, with his handle beside his id.
fn fixture() -> FakeState {
    let mut state = FakeState::default();
    state.history.insert(
        TEAM.to_owned(),
        vec![
            profiled(&ts(10, 0), "U0ADA0001", "Ada Lovelace", "morning"),
            plain(
                &ts(20, 0),
                "U0ADA0001",
                "<@U0GRACE01> can you review? cc <@U0ADA0001>",
            ),
            plain(&ts(30, 0), "U0ADA0001", "who is <@U0GHOST01>?"),
            plain(
                &ts(40, 0),
                "U0ADA0001",
                "ask <@W0ALAN001|alan> in <#C0000OPS01>",
            ),
            plain(
                &ts(50, 0),
                "U0ADA0001",
                "<!subteam^S0123ABC|@oncall> see <#C0000OPS01|> and <#C0000GONE1|>",
            ),
        ],
    );
    state.history.insert(OPS.to_owned(), Vec::new());
    state
        .conversation_names
        .insert(OPS.to_owned(), "ops-room".to_owned());
    state.users.insert(
        "U0ADA0001".to_owned(),
        user("U0ADA0001", "Ada Lovelace", "Augusta Ada King"),
    );
    state.users.insert(
        "U0GRACE01".to_owned(),
        user("U0GRACE01", "", "Grace Hopper"),
    );
    state
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
async fn every_read_says_who_each_mention_names_and_keeps_the_mention() {
    let fake = Fake::start(fixture(), 20).await;
    let client = fake.client();
    let messages = client
        .fetch_page(&ChannelId(TEAM.to_owned()), 20, None, None)
        .await
        .expect("a page");
    assert_eq!(messages.len(), 5);
    let review = &messages[1];
    assert_eq!(
        review.content, "<@U0GRACE01> can you review? cc <@U0ADA0001>",
        "the body keeps the mentions as written, so a reply that quotes them notifies"
    );
    assert_eq!(
        review.mentions,
        [
            named("U0GRACE01", "Grace Hopper"),
            named("U0ADA0001", "Ada Lovelace")
        ]
    );
    // An id nobody can name is left out, and so drawn as its id.
    assert_eq!(messages[2].content, "who is <@U0GHOST01>?");
    assert!(
        messages[2].mentions.is_empty(),
        "{:?}",
        messages[2].mentions
    );
    // A legacy mention is named by its own label, with no call; an unnamed channel mention by the
    // name Slack reports for it.
    assert_eq!(messages[3].content, "ask <@W0ALAN001> in #ops-room");
    assert_eq!(messages[3].mentions, [named("W0ALAN001", "alan")]);
    // A user group keeps its handle. A channel with an empty label is named like one with none,
    // and one Slack cannot find keeps its id.
    assert_eq!(messages[4].content, "@oncall see #ops-room and #C0000GONE1");
    // One lookup per person and per channel, however often they are mentioned.
    let looked_up: Vec<String> = fake
        .requests("users.info")
        .into_iter()
        .filter_map(|request| request.get("user").cloned())
        .collect();
    assert_eq!(looked_up.len(), 3, "{looked_up:?}");
    for id in ["U0ADA0001", "U0GRACE01", "U0GHOST01"] {
        assert!(looked_up.iter().any(|asked| asked == id), "{looked_up:?}");
    }
    assert!(
        !looked_up.iter().any(|asked| asked == "W0ALAN001"),
        "a legacy mention names itself"
    );
    let mut channels: Vec<String> = fake
        .requests("conversations.info")
        .into_iter()
        .filter_map(|request| request.get("channel").cloned())
        .collect();
    channels.sort();
    assert_eq!(channels, ["C0000GONE1", OPS]);

    // Read again: every name, found or not, is remembered.
    let again = client
        .fetch_page(&ChannelId(TEAM.to_owned()), 20, None, None)
        .await
        .expect("a page");
    assert_eq!(again, messages);
    assert_eq!(fake.calls("users.info"), 3);
    assert_eq!(fake.calls("conversations.info"), 2);
    assert_no_stray_methods(&fake);
}

#[tokio::test]
async fn one_read_looks_up_a_bounded_number_of_strangers() {
    let mut state = FakeState::default();
    let crowd: Vec<String> = (0..40).map(|n| format!("U0CROWD{n:02}")).collect();
    let text = crowd
        .iter()
        .map(|id| format!("<@{id}>"))
        .collect::<Vec<_>>()
        .join(" ");
    for id in &crowd {
        state
            .users
            .insert(id.clone(), user(id, &format!("Person {id}"), ""));
    }
    state.history.insert(
        TEAM.to_owned(),
        vec![profiled(&ts(10, 0), "U0ADA0001", "Ada Lovelace", &text)],
    );
    let fake = Fake::start(state, 20).await;
    let client = fake.client();
    let first = client
        .fetch_page(&ChannelId(TEAM.to_owned()), 20, None, None)
        .await
        .expect("a page");
    assert_eq!(fake.calls("users.info"), 25, "at most 25 strangers a read");
    assert_eq!(first[0].mentions.len(), 25);
    // The next read names the rest, and looks up only them.
    let second = client
        .fetch_page(&ChannelId(TEAM.to_owned()), 20, None, None)
        .await
        .expect("a page");
    assert_eq!(fake.calls("users.info"), 40);
    assert_eq!(second[0].mentions.len(), 40);
    assert_eq!(
        second[0].mentions[0],
        named("U0CROWD00", "Person U0CROWD00")
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

[[channels]]
id = "{TEAM}"
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

fn by_content<'v>(page: &'v Value, content: &str) -> &'v Value {
    page["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["content"] == content)
        .unwrap_or_else(|| panic!("no message {content:?}: {page}"))
}

#[tokio::test]
async fn the_page_draws_and_speech_says_the_name_and_an_unknown_id_stays_an_id() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake).await;
    let (status, page) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{TEAM}/page?render=html"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");

    let review = by_content(&page, "<@U0GRACE01> can you review? cc <@U0ADA0001>");
    let html = review["content_html"]
        .as_str()
        .unwrap_or_else(|| panic!("not rendered: {review}"));
    assert!(
        html.contains(r#"<span class="mention">@Grace Hopper</span> can you review?"#),
        "{html}"
    );
    assert!(
        html.contains(r#"cc <span class="mention">@Ada Lovelace</span>"#),
        "{html}"
    );
    assert!(!html.contains("U0GRACE01"), "the id is not drawn: {html}");
    let said = review["spoken_content"].as_str().expect("prepared");
    assert_eq!(said, "Grace Hopper can you review? cc Ada Lovelace");
    // The names are drawn from the server's own rendering: the page is sent no second copy.
    assert!(review.get("mentions").is_none(), "{review}");

    // The negative control: an id nobody could name is drawn by id and said as a mention.
    let ghost = by_content(&page, "who is <@U0GHOST01>?");
    assert!(
        ghost["content_html"]
            .as_str()
            .expect("rendered")
            .contains(r#"<span class="mention">@U0GHOST01</span>"#),
        "{ghost}"
    );
    assert_eq!(ghost["spoken_content"], "who is a mention?");

    let alan = by_content(&page, "ask <@W0ALAN001> in #ops-room");
    assert!(
        alan["content_html"]
            .as_str()
            .expect("rendered")
            .contains(r#"<span class="mention">@alan</span> in #ops-room"#),
        "{alan}"
    );
    let group = by_content(&page, "@oncall see #ops-room and #C0000GONE1");
    assert!(
        group["spoken_content"]
            .as_str()
            .unwrap_or_else(|| group["content"].as_str().expect("content"))
            .contains("oncall see #ops-room"),
        "{group}"
    );

    // Flat, which the page reads too, draws the same.
    let (status, flat) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{TEAM}/timeline?view=flat&limit=20&render=html"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{flat}");
    assert!(
        by_content(&flat, "<@U0GRACE01> can you review? cc <@U0ADA0001>")["content_html"]
            .as_str()
            .expect("rendered")
            .contains("@Grace Hopper"),
        "{flat}"
    );
}

#[tokio::test]
async fn a_reply_that_quotes_a_mention_posts_the_mention() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake).await;
    let (status, body) = call(
        &state,
        "POST",
        &format!("/api/v1/channels/{TEAM}/reply"),
        Some(json!({"text": "<@U0GRACE01> thanks"})),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    let posted = fake.requests("chat.postMessage");
    assert_eq!(
        posted
            .last()
            .and_then(|request| request.get("text"))
            .map(String::as_str),
        Some("<@U0GRACE01> thanks"),
        "what is posted is the mention, which notifies, never the name"
    );
}
