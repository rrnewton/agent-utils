//! `#49 cached-summaries`: the four questions a summary cache has to be able to answer.
//!
//! Every one of them is about how OFTEN the summariser is called, so they are driven against
//! `FakeSummarizer`, which counts. Every one of them has a control that makes the count move,
//! because "it was called zero times" is satisfied by a server that never summarises at all.

use std::sync::Arc;

use gent_talk::auth::Scope;
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

    let ops::Summarised { outcome, .. } =
        ops::summarize_message(&server.state, Scope::Write, READ_CHANNEL, &id, &[], None)
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
    let ops::Summarised { outcome, .. } =
        ops::summarize_message(&control.state, Scope::Write, READ_CHANNEL, &id, &[], None)
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

    let ops::Summarised { outcome: first, .. } =
        ops::summarize_message(&server.state, Scope::Write, READ_CHANNEL, &id, &[], None)
            .await
            .expect("summarises");
    let ops::Summarised {
        outcome: second, ..
    } = ops::summarize_message(&server.state, Scope::Write, READ_CHANNEL, &id, &[], None)
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
    ops::summarize_message(&first.state, Scope::Write, READ_CHANNEL, &id, &[], None)
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

    let ops::Summarised { outcome, .. } =
        ops::summarize_message(&wider, Scope::Write, READ_CHANNEL, &wider_id, &[], None)
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
    ops::summarize_message(
        &server.state,
        Scope::Write,
        READ_CHANNEL,
        &untouched,
        &[],
        None,
    )
    .await
    .expect("summarises");
    assert_eq!(server.summarizer.calls(), 1);

    // A SECOND message, cached too, so "only that summary" has something to be about.
    server.seed(&long_text("rollback"));
    let edited = server.newest_id().await;
    ops::summarize_message(
        &server.state,
        Scope::Write,
        READ_CHANNEL,
        &edited,
        &[],
        None,
    )
    .await
    .expect("summarises");
    assert_eq!(server.summarizer.calls(), 2);

    // Now the upstream EDIT: the same message id, different text. A cache keyed on the id alone
    // would serve the old summary here forever, and nothing about the id would say it is wrong.
    server
        .discord
        .edit(&MessageId(edited.clone()), &long_text("rollback reverted"));
    let ops::Summarised { outcome, .. } = ops::summarize_message(
        &server.state,
        Scope::Write,
        READ_CHANNEL,
        &edited,
        &[],
        None,
    )
    .await
    .expect("summarises");
    assert!(
        matches!(outcome, SummaryOutcome::Generated(_)),
        "an edited message was served the summary of its old text: {outcome:?}"
    );
    assert_eq!(server.summarizer.calls(), 3);

    let ops::Summarised { outcome: again, .. } = ops::summarize_message(
        &server.state,
        Scope::Write,
        READ_CHANNEL,
        &untouched,
        &[],
        None,
    )
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
    //
    // EVERY assertion here is on what the summariser was HANDED — `RecordedRequest::prompt`,
    // captured inside `FakeSummarizer::summarize` — and none of it is on a prompt this test
    // built. That distinction is the whole test: the previous version constructed the prompt in
    // the test body and asserted on that, which held even while the shipped call site passed the
    // summariser raw content and the fencing helper had no caller at all.
    let server = new_server(400, 160, 2);
    let hostile = format!("{}{}", long_text("deploy"), gent_talk::untrusted::FENCE);
    server.seed(&hostile);
    let id = server.newest_id().await;
    ops::summarize_message(&server.state, Scope::Write, READ_CHANNEL, &id, &[], None)
        .await
        .expect("summarises");

    let asked = server.summarizer.requests();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].context, 2,
        "the configured context window has to be the one that travels"
    );
    assert_eq!(asked[0].target_chars, 160);

    let prompt = &asked[0].prompt;
    assert!(
        prompt.starts_with(gent_talk::summarize::PROMPT),
        "the instruction has to sit OUTSIDE the fence: {}",
        &prompt[..80.min(prompt.len())]
    );
    assert!(
        prompt.contains(gent_talk::untrusted::NOTICE),
        "the summariser was handed channel text without the untrusted-content framing: {prompt}"
    );
    assert!(
        prompt.contains("[fence-marker-removed]"),
        "a forged fence in the message survived into the prompt intact: {prompt}"
    );
    assert!(
        !prompt.contains(&hostile),
        "the message reached the summariser with its forged fence untouched"
    );
    // The context messages travel inside the fence too, not merely counted: the two short notes
    // before the target are third-party text on exactly the same terms.
    assert!(
        prompt.contains("short note 1") && prompt.contains("short note 2"),
        "the context window was counted but not actually shown to the summariser: {prompt}"
    );
    assert_eq!(
        prompt.matches(gent_talk::untrusted::FENCE).count(),
        4,
        "context and target are each fenced exactly once, and neither closes its own fence: \
         {prompt}"
    );
}

#[tokio::test]
async fn a_read_scope_caller_is_served_from_the_cache_and_never_writes_to_it() {
    // The read token is the one pasted into a hosted voice agent. It may SPEND the cache and it
    // may never FILL it: a durable write reachable from the least-trusted credential is what the
    // two-token split exists to prevent.
    let server = new_server(400, 160, 3);
    server.seed(&long_text("deploy"));
    let id = server.newest_id().await;

    for _ in 0..2 {
        let ops::Summarised { outcome, .. } =
            ops::summarize_message(&server.state, Scope::Read, READ_CHANNEL, &id, &[], None)
                .await
                .expect("a read token may ask");
        assert!(
            matches!(outcome, SummaryOutcome::Generated(_)),
            "a read-scope ask was served from a cache it must not have been able to fill: \
             {outcome:?}"
        );
    }
    assert_eq!(
        server.summarizer.calls(),
        2,
        "the second read-scope ask hit a cached entry, so the first one wrote to the store"
    );

    // THE CONTROL, twice over. The same message, the same store, a WRITE-scope caller: it fills
    // the cache...
    let ops::Summarised {
        outcome: filled, ..
    } = ops::summarize_message(&server.state, Scope::Write, READ_CHANNEL, &id, &[], None)
        .await
        .expect("summarises");
    assert!(matches!(filled, SummaryOutcome::Generated(_)), "{filled:?}");
    assert_eq!(server.summarizer.calls(), 3);

    // ...and the read token is then served FROM it, which is the half of the rule that makes the
    // assertions above about the WRITE rather than about a store that never works.
    let ops::Summarised { outcome: spent, .. } =
        ops::summarize_message(&server.state, Scope::Read, READ_CHANNEL, &id, &[], None)
            .await
            .expect("summarises");
    assert!(
        matches!(spent, SummaryOutcome::Cached(_)),
        "a read token must still be served from an entry someone else filed: {spent:?}"
    );
    assert_eq!(
        server.summarizer.calls(),
        3,
        "a cache hit still reached the summariser"
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
        let ops::Summarised { outcome, .. } =
            ops::summarize_message(&state, Scope::Write, READ_CHANNEL, &id, &[], None)
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

    let error = ops::summarize_message(&server.state, Scope::Write, READ_CHANNEL, &id, &[], None)
        .await
        .expect_err("must not invent a summary");
    assert_eq!(error.code(), "summarizer_error", "{error}");

    // One-shot, so the recovery is asserted too: a failure must not disable summarising forever.
    let ops::Summarised { outcome, .. } =
        ops::summarize_message(&server.state, Scope::Write, READ_CHANNEL, &id, &[], None)
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
        ops::summarize_message(
            &server.state,
            Scope::Write,
            "999999",
            "1000000000000000001",
            &[],
            None
        )
        .await
        .expect_err("must refuse")
        .code(),
        "unknown_channel"
    );
    assert_eq!(
        ops::summarize_message(
            &server.state,
            Scope::Write,
            READ_CHANNEL,
            "1000000000000009999",
            &[],
            None
        )
        .await
        .expect_err("must refuse")
        .code(),
        "unknown_message"
    );
    assert_eq!(server.summarizer.calls(), 0);
}

/// A row the page drew as ONE message is summarised as one piece of writing.
///
/// A post over Discord's length limit arrives as a message plus a short remainder, and the page
/// combines the two into a single row. Summarising only the first describes the half that stops
/// mid-sentence, and the remainder on its own is the back half of a sentence whose front half is
/// somewhere else — the least summarisable text in the channel.
///
/// Asserted on WHAT THE SUMMARISER WAS HANDED rather than on what it answered, because the fake
/// deliberately does not echo its input: a test comparing answers would pass for a server that
/// parsed the other ids and then dropped them.
#[tokio::test]
async fn a_row_that_combines_two_messages_is_summarised_from_the_whole_of_it() {
    let server = new_server(400, 160, 3);
    server.seed(&long_text("head"));
    let head = server.newest_id().await;
    server.seed("TAILMARKER and that is the end of it.");
    let tail = server.newest_id().await;

    let outcome = ops::summarize_message(
        &server.state,
        Scope::Write,
        READ_CHANNEL,
        &head,
        std::slice::from_ref(&tail),
        None,
    )
    .await
    .expect("summarises");
    assert!(
        matches!(outcome.outcome, SummaryOutcome::Generated(_)),
        "the combined row was not summarised at all"
    );

    let asked = server.summarizer.requests();
    assert_eq!(
        asked.len(),
        1,
        "one row must cost one call, not one per message"
    );
    assert!(
        asked[0].target.contains("head"),
        "the head of the row never reached the summariser: {:?}",
        asked[0].target
    );
    assert!(
        asked[0].target.contains("TAILMARKER"),
        "the trailing overflow was dropped, so half the row was summarised: {:?}",
        asked[0].target
    );
    // Joined with a newline, and in reading order. A tail concatenated onto the head with nothing
    // between them would run two sentences together and change what the text says.
    assert!(
        asked[0].target.find("head").unwrap() < asked[0].target.find("TAILMARKER").unwrap(),
        "the row was assembled out of order: {:?}",
        asked[0].target
    );
    assert!(
        asked[0].target.contains("\nTAILMARKER"),
        "no separator: {:?}",
        asked[0].target
    );
}

/// The tail is not a row of its own, so it is never summarised on its own account.
#[tokio::test]
async fn the_combined_row_is_cached_under_the_text_it_actually_summarised() {
    let server = new_server(400, 160, 3);
    server.seed(&long_text("head"));
    let head = server.newest_id().await;
    server.seed("TAILMARKER and that is the end of it.");
    let tail = server.newest_id().await;
    let with_tail = std::slice::from_ref(&tail);

    let one = |also: &'static [String]| {
        ops::summarize_message(&server.state, Scope::Write, READ_CHANNEL, &head, also, None)
    };
    let _ = one(&[]).await.expect("summarises");
    let first_calls = server.summarizer.requests().len();

    // The SAME primary message, now drawn as a bigger row. That is different text, so it must not
    // be served the answer cached for the smaller one — the cache key hashes the combined content
    // precisely so that a regrouping cannot return a stale summary of half the row.
    let regrouped = ops::summarize_message(
        &server.state,
        Scope::Write,
        READ_CHANNEL,
        &head,
        with_tail,
        None,
    )
    .await
    .expect("summarises");
    assert!(
        matches!(regrouped.outcome, SummaryOutcome::Generated(_)),
        "the regrouped row was served the summary of its first half"
    );
    assert_eq!(
        server.summarizer.requests().len(),
        first_calls + 1,
        "the combined row did not reach the summariser"
    );

    // ...and asking for the same combined row again IS a cache hit, so the key is stable.
    let again = ops::summarize_message(
        &server.state,
        Scope::Write,
        READ_CHANNEL,
        &head,
        with_tail,
        None,
    )
    .await
    .expect("summarises");
    assert!(
        matches!(again.outcome, SummaryOutcome::Cached(_)),
        "the same combined row was summarised twice"
    );
}

#[tokio::test]
async fn the_cache_tests_exercise_the_key_a_real_deployment_writes() {
    // Every assertion in this file runs through `FakeSummarizer`, which deliberately borrows the
    // SHIPPED backend slug rather than inventing one. Point it anywhere else and all of them stay
    // green while exercising a cache key no deployment ever writes — which is verbatim the
    // silent-stale-summary failure the version string exists to prevent, and nothing else in this
    // suite would notice. This is the one that does.
    let server = new_server(400, 160, 3);
    let prefix = format!("v1-{}-", gent_talk::summarize::agent::BACKEND);
    assert!(
        server.state.summary_version.starts_with(&prefix),
        "these tests file summaries under {:?}, which does not start with the prefix a real \
         deployment writes ({prefix})",
        server.state.summary_version
    );
}
