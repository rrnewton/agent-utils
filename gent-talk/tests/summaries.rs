//! `#49 cached-summaries`: the four questions a summary cache has to be able to answer.
//!
//! Every one of them is about how OFTEN the summariser is called, so they are driven against
//! `FakeSummarizer`, which counts. Every one of them has a control that makes the count move,
//! because "it was called zero times" is satisfied by a server that never summarises at all.

use std::sync::Arc;

use gent_talk::discord::fake::FakeDiscord;
use gent_talk::model::{ChannelId, MessageId};
use gent_talk::ops::{self, SummaryOutcome};
use gent_talk::state::AppState;
use gent_talk::store::fake::FakeStore;
use gent_talk::summarize::fake::FakeSummarizer;
use gent_talk::testing::{self, READ_CHANNEL};

/// A configuration with the summary thresholds a test chose.
fn toml_with(threshold: usize, target: usize, context: usize) -> String {
    format!(
        "{}\n[summaries]\nthreshold_chars = {threshold}\ntarget_chars = {target}\n\
         context_messages = {context}\n",
        testing::config_toml()
    )
}

struct Server {
    state: AppState,
    discord: Arc<FakeDiscord>,
    store: Arc<FakeStore>,
    summarizer: Arc<FakeSummarizer>,
    channel: ChannelId,
}

impl Server {
    fn seed(&self, content: &str) {
        self.discord.seed(&self.channel, "codex-eng", content);
    }

    async fn newest_id(&self) -> String {
        newest_id(&self.state).await
    }
}

fn new_server(threshold: usize, target: usize, context: usize) -> Server {
    let (state, discord, store, summarizer) =
        testing::state_with_summarizer(&toml_with(threshold, target, context));
    let channel = ChannelId(READ_CHANNEL.to_owned());
    // Three short neighbours, so a context window has something to pick up.
    for n in 0..3 {
        discord.seed(&channel, "codex-eng", &format!("short note {n}"));
    }
    Server {
        state,
        discord,
        store,
        summarizer,
        channel,
    }
}

/// A message comfortably over any threshold used here.
fn long_text(tag: &str) -> String {
    format!("{tag} ").repeat(120)
}

async fn newest_id(state: &AppState) -> String {
    ops::messages(state, READ_CHANNEL, None)
        .await
        .expect("reads")
        .messages
        .last()
        .expect("a message")
        .id
        .0
        .clone()
}

#[tokio::test]
async fn a_short_message_costs_no_summariser_call_at_all() {
    // The threshold is checked BEFORE the cache and before the summariser. A page that asked
    // about every row would otherwise spend a model call on every one-line message in a channel.
    let server = new_server(400, 160, 3);
    server.seed("the runner stalled");
    let id = server.newest_id().await;

    let (_channel, outcome) = ops::summarize_message(&server.state, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");
    assert_eq!(outcome, SummaryOutcome::BelowThreshold);
    assert_eq!(
        server.summarizer.calls(),
        0,
        "a message shorter than the threshold reached the summariser anyway"
    );

    // THE CONTROL. The same message, the same code path, a threshold it clears — and now it IS
    // summarised. Without this the assertion above is satisfied by a server that summarises
    // nothing.
    let control = new_server(5, 4, 3);
    control.seed("the runner stalled");
    let id = control.newest_id().await;
    let (_channel, outcome) = ops::summarize_message(&control.state, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");
    assert!(
        matches!(outcome, SummaryOutcome::Generated(_)),
        "the control must actually summarise: {outcome:?}"
    );
    assert_eq!(control.summarizer.calls(), 1);
}

#[tokio::test]
async fn asking_twice_generates_once() {
    let server = new_server(400, 160, 3);
    server.seed(&long_text("deploy"));
    let id = server.newest_id().await;

    let (_c, first) = ops::summarize_message(&server.state, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");
    let (_c, second) = ops::summarize_message(&server.state, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");

    assert!(matches!(first, SummaryOutcome::Generated(_)), "{first:?}");
    assert!(
        matches!(second, SummaryOutcome::Cached(_)),
        "the second ask must be served from the store: {second:?}"
    );
    assert_eq!(first.text(), second.text(), "the cache changed the answer");
    assert_eq!(
        server.summarizer.calls(),
        1,
        "the second ask reached the summariser, so nothing is being cached"
    );
}

#[tokio::test]
async fn changing_a_summary_setting_makes_every_old_summary_unreachable() {
    // THE CONTROL for the test above, and the whole reason `policy_version` exists. The same
    // message, the same store, a different target width — and the cached entry must NOT be
    // served, because it was produced under an instruction that no longer applies.
    let first = new_server(400, 160, 3);
    first.seed(&long_text("deploy"));
    let id = first.newest_id().await;
    ops::summarize_message(&first.state, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");
    assert_eq!(first.summarizer.calls(), 1);

    // A second server over THE SAME store, differing only in target_chars.
    let wider_server = new_server(400, 161, 3);
    let mut wider = wider_server.state.clone();
    wider.store = first.store.clone();
    wider_server.seed(&long_text("deploy"));
    let wider_summarizer = wider_server.summarizer.clone();
    let wider_id = newest_id(&wider).await;

    let (_c, outcome) = ops::summarize_message(&wider, READ_CHANNEL, &wider_id, None)
        .await
        .expect("summarises");
    assert!(
        matches!(outcome, SummaryOutcome::Generated(_)),
        "a changed policy served a summary produced under the old one: {outcome:?}"
    );
    assert_eq!(wider_summarizer.calls(), 1);
}

#[tokio::test]
async fn editing_the_message_upstream_regenerates_only_that_summary() {
    let server = new_server(400, 160, 3);
    server.seed(&long_text("deploy"));
    let untouched = server.newest_id().await;
    ops::summarize_message(&server.state, READ_CHANNEL, &untouched, None)
        .await
        .expect("summarises");
    assert_eq!(server.summarizer.calls(), 1);

    // A SECOND message, cached too, so "only that summary" has something to be about.
    server.seed(&long_text("rollback"));
    let edited = server.newest_id().await;
    ops::summarize_message(&server.state, READ_CHANNEL, &edited, None)
        .await
        .expect("summarises");
    assert_eq!(server.summarizer.calls(), 2);

    // Now the upstream EDIT: the same message id, different text. A cache keyed on the id alone
    // would serve the old summary here forever, and nothing about the id would say it is wrong.
    server
        .discord
        .edit(&MessageId(edited.clone()), &long_text("rollback reverted"));
    let (_c, outcome) = ops::summarize_message(&server.state, READ_CHANNEL, &edited, None)
        .await
        .expect("summarises");
    assert!(
        matches!(outcome, SummaryOutcome::Generated(_)),
        "an edited message was served the summary of its old text: {outcome:?}"
    );
    assert_eq!(server.summarizer.calls(), 3);

    let (_c, again) = ops::summarize_message(&server.state, READ_CHANNEL, &untouched, None)
        .await
        .expect("summarises");
    assert!(
        matches!(again, SummaryOutcome::Cached(_)),
        "the untouched message's summary was invalidated too: {again:?}"
    );
    assert_eq!(server.summarizer.calls(), 3);
}

#[tokio::test]
async fn the_summariser_is_shown_fenced_text_and_the_context_it_was_configured_for() {
    // A summariser is a model being fed channel text. Being short is not an exemption from the
    // boundary the MCP path already enforces.
    let server = new_server(400, 160, 2);
    server.seed(&format!(
        "{}{}",
        long_text("deploy"),
        gent_talk::untrusted::FENCE
    ));
    let id = server.newest_id().await;
    ops::summarize_message(&server.state, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");

    let asked = server.summarizer.requests();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].context, 2,
        "the configured context window has to be the one that travels"
    );
    assert_eq!(asked[0].target_chars, 160);

    let window = ops::messages(&server.state, READ_CHANNEL, None)
        .await
        .expect("reads");
    let position = window
        .messages
        .iter()
        .position(|m| m.id.as_str() == id)
        .expect("the message");
    let request = gent_talk::summarize::SummaryRequest {
        target: &window.messages[position],
        context: &window.messages[position - 2..position],
        target_chars: 160,
    };
    let prompt = ops::summary_prompt(&request);
    assert!(
        prompt.starts_with(gent_talk::summarize::PROMPT),
        "the instruction has to sit OUTSIDE the fence: {}",
        &prompt[..80.min(prompt.len())]
    );
    assert!(
        prompt.contains(gent_talk::untrusted::NOTICE),
        "the summariser was handed channel text without the untrusted-content framing"
    );
    assert!(
        prompt.contains("[fence-marker-removed]"),
        "a forged fence in the message survived into the prompt intact"
    );
}

#[tokio::test]
async fn a_store_that_cannot_cache_degrades_to_generating_rather_than_refusing() {
    // The cache is an optimisation. Making a READ fail because the OPTIONAL durable store is not
    // configured would turn storage into a hard dependency of summarising, which it is not.
    let server = new_server(400, 160, 3);
    let mut state = server.state.clone();
    state.store = Arc::new(gent_talk::store::disabled::DisabledStore);
    server.seed(&long_text("deploy"));
    let id = newest_id(&state).await;

    for _ in 0..2 {
        let (_c, outcome) = ops::summarize_message(&state, READ_CHANNEL, &id, None)
            .await
            .expect("an unconfigured store must not make a read fail");
        assert!(
            matches!(outcome, SummaryOutcome::Generated(_)),
            "{outcome:?}"
        );
    }
    assert_eq!(
        server.summarizer.calls(),
        2,
        "with no cache every ask must really generate; a 1 here would mean it cached somewhere"
    );
}

#[tokio::test]
async fn a_summariser_that_refuses_is_reported_rather_than_papered_over() {
    let server = new_server(400, 160, 3);
    server.seed(&long_text("deploy"));
    let id = server.newest_id().await;
    server.summarizer.fail_next("the model host is down");

    let error = ops::summarize_message(&server.state, READ_CHANNEL, &id, None)
        .await
        .expect_err("must not invent a summary");
    assert_eq!(error.code(), "summarizer_error", "{error}");

    // One-shot, so the recovery is asserted too: a failure must not disable summarising forever.
    let (_c, outcome) = ops::summarize_message(&server.state, READ_CHANNEL, &id, None)
        .await
        .expect("summarises");
    assert!(
        matches!(outcome, SummaryOutcome::Generated(_)),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_message_outside_the_window_or_the_allowlist_is_refused_before_anything_is_summarised() {
    let server = new_server(400, 160, 3);
    assert_eq!(
        ops::summarize_message(&server.state, "999999", "1000000000000000001", None)
            .await
            .expect_err("must refuse")
            .code(),
        "unknown_channel"
    );
    assert_eq!(
        ops::summarize_message(&server.state, READ_CHANNEL, "1000000000000009999", None)
            .await
            .expect_err("must refuse")
            .code(),
        "unknown_message"
    );
    assert_eq!(server.summarizer.calls(), 0);
}
