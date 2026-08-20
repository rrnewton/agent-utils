//! `#50 todo-view`: the messages the owner has not dealt with, and the two acts that change that.
//!
//! Every test here is about the OVERLAY — the state Discord does not hold and this server
//! authors. `#61 unread-status` settled that it is ours: no sync-in, no sync-back, single-tenant.
//! So there is nothing upstream to reconcile against, and what these tests are actually asserting
//! is that a declaration made here is the one the reader gets back, exactly, including when it is
//! undone.
//!
//! What is deliberately NOT here: a message leaving the list because it was REPLIED to. That is
//! derived state and it needs a reply reference on `model::Message`, which is a wire-format change
//! touching every struct literal that builds one. It is a separate change and a separate issue;
//! see the note above `ops::todo`.

use std::sync::Arc;

use gent_talk::model::{ChannelId, MessageId};
use gent_talk::ops;
use gent_talk::state::AppState;
use gent_talk::store::fake::FakeStore;
use gent_talk::store::{Retention, StateStore as _};
use gent_talk::testing::{self, READ_CHANNEL};

struct Server {
    state: AppState,
    store: Arc<FakeStore>,
    channel: ChannelId,
    ids: Vec<MessageId>,
}

impl Server {
    /// A channel with `count` messages in it, oldest first, plus the store behind it.
    fn with(count: usize) -> Self {
        Self::with_retention(count, Retention::default())
    }

    fn with_retention(count: usize, retention: Retention) -> Self {
        let store = Arc::new(FakeStore::with_retention(retention));
        let (state, discord) = testing::state_with(store.clone());
        let channel = ChannelId(READ_CHANNEL.to_owned());
        let ids = (0..count)
            .map(|n| discord.seed(&channel, "codex-eng", &format!("message {n}")))
            .collect();
        Self {
            state,
            store,
            channel,
            ids,
        }
    }

    async fn left(&self) -> Vec<String> {
        ops::todo(&self.state, READ_CHANNEL, None)
            .await
            .expect("reads")
            .messages
            .iter()
            .map(|m| m.id.0.clone())
            .collect()
    }

    fn id(&self, n: usize) -> MessageId {
        self.ids[n].clone()
    }
}

#[tokio::test]
async fn everything_is_wanting_attention_until_somebody_says_otherwise() {
    // The floor. A to-do view that started empty would look like a working filter and would be a
    // view of nothing.
    let server = Server::with(4);
    let view = ops::todo(&server.state, READ_CHANNEL, None)
        .await
        .expect("reads");
    assert_eq!(view.messages.len(), 4);
    assert_eq!(view.window, 4, "the window size has to be reported as well");
    assert!(
        view.complete,
        "four messages inside a fifty-message ceiling IS the whole channel"
    );
}

#[tokio::test]
async fn dismissing_one_message_removes_exactly_that_one() {
    let server = Server::with(4);
    let change = ops::dismiss(&server.state, READ_CHANNEL, &[server.id(2)])
        .await
        .expect("dismisses");
    assert_eq!(change.messages, vec![server.id(2)]);

    let left = server.left().await;
    assert_eq!(
        left,
        vec![
            server.ids[0].0.clone(),
            server.ids[1].0.clone(),
            server.ids[3].0.clone()
        ],
        "a dismissal took the wrong messages out of the list"
    );
    // ...and the WINDOW still says how much there was, so an interface can say "3 of 4 left"
    // rather than implying the channel has three messages in it.
    let view = ops::todo(&server.state, READ_CHANNEL, None)
        .await
        .expect("reads");
    assert_eq!(view.window, 4);
}

#[tokio::test]
async fn undo_puts_back_exactly_what_was_taken_and_nothing_else() {
    // Archiving is easy to do by accident on a moving list, and a gesture with no way back is one
    // people learn to approach cautiously — which defeats the point of a fast list.
    let server = Server::with(4);
    ops::dismiss(&server.state, READ_CHANNEL, &[server.id(1), server.id(2)])
        .await
        .expect("dismisses");
    assert_eq!(server.left().await.len(), 2);

    let change = ops::restore(&server.state, READ_CHANNEL, &[server.id(2)])
        .await
        .expect("restores");
    assert_eq!(change.messages, vec![server.id(2)]);
    assert_eq!(
        server.left().await,
        vec![
            server.ids[0].0.clone(),
            server.ids[2].0.clone(),
            server.ids[3].0.clone()
        ],
        "undo restored the wrong message, or restored more than one"
    );
    // The other dismissal is untouched: an undo of one act must not undo the act before it.
    assert!(
        !server.left().await.contains(&server.ids[1].0),
        "undoing one dismissal resurrected another"
    );
}

#[tokio::test]
async fn bankruptcy_clears_everything_through_the_boundary_and_nothing_after_it() {
    // Tested AT the boundary, because "before" is exactly the word that hides an off-by-one: the
    // row the reader pressed is part of what they are giving up on, and a version that excluded it
    // would leave one message behind on every single use, which reads as the feature not working.
    let server = Server::with(5);
    let change = ops::declare_bankruptcy(&server.state, READ_CHANNEL, &server.id(2))
        .await
        .expect("declares");

    assert_eq!(
        change.messages,
        vec![server.id(0), server.id(1), server.id(2)],
        "the boundary message itself must be cleared, and nothing past it"
    );
    assert_eq!(
        server.left().await,
        vec![server.ids[3].0.clone(), server.ids[4].0.clone()],
        "bankruptcy cleared the wrong side of the boundary"
    );
}

#[tokio::test]
async fn bankruptcy_at_the_newest_message_clears_the_whole_backlog_and_undo_brings_it_all_back() {
    // The other edge, and the ordinary use: the reader gives up on everything they can see.
    let server = Server::with(5);
    let change = ops::declare_bankruptcy(&server.state, READ_CHANNEL, &server.id(4))
        .await
        .expect("declares");
    assert_eq!(change.messages.len(), 5);
    assert!(server.left().await.is_empty());

    // Bulk AND destructive, so the undo has to be exact rather than approximate.
    ops::restore(&server.state, READ_CHANNEL, &change.messages)
        .await
        .expect("restores");
    assert_eq!(server.left().await.len(), 5, "the undo lost messages");
}

#[tokio::test]
async fn bankruptcy_reports_only_what_it_really_cleared_so_the_undo_cannot_resurrect_older_work() {
    // THE trap in a bulk undo. The reader dealt with one message an hour ago and then declared
    // bankruptcy through a later one. If the bankruptcy claimed the earlier message too, undoing
    // it would put back something the reader cleared deliberately and never asked to see again —
    // and nothing on the screen would explain where it came from.
    let server = Server::with(5);
    ops::dismiss(&server.state, READ_CHANNEL, &[server.id(1)])
        .await
        .expect("dismisses");

    let change = ops::declare_bankruptcy(&server.state, READ_CHANNEL, &server.id(3))
        .await
        .expect("declares");
    assert_eq!(
        change.messages,
        vec![server.id(0), server.id(2), server.id(3)],
        "bankruptcy claimed a message it did not change: {:?}",
        change.messages
    );

    ops::restore(&server.state, READ_CHANNEL, &change.messages)
        .await
        .expect("restores");
    assert!(
        !server.left().await.contains(&server.ids[1].0),
        "undoing the bankruptcy resurrected a message dismissed before it"
    );
    assert_eq!(server.left().await.len(), 4);
}

#[tokio::test]
async fn dismissing_the_same_message_twice_is_not_an_error_and_changes_nothing() {
    // A control the reader cannot see the result of gets tapped twice. The second tap must be a
    // no-op, or one undo would leave the message dealt with.
    let server = Server::with(3);
    ops::dismiss(&server.state, READ_CHANNEL, &[server.id(0)])
        .await
        .expect("dismisses");
    ops::dismiss(&server.state, READ_CHANNEL, &[server.id(0)])
        .await
        .expect("dismisses again");
    assert_eq!(server.left().await.len(), 2);

    ops::restore(&server.state, READ_CHANNEL, &[server.id(0)])
        .await
        .expect("restores");
    assert_eq!(
        server.left().await.len(),
        3,
        "one undo did not undo a doubled dismissal"
    );
}

#[tokio::test]
async fn a_message_outside_the_window_or_the_allowlist_cannot_grow_the_store() {
    // The same guard `ops::allowed` puts on every channel operation, one level down: an invented
    // snowflake must not be able to add a row per request to a durable table.
    let server = Server::with(2);
    assert_eq!(
        ops::dismiss(
            &server.state,
            "999999",
            &[MessageId("1000000000000000001".to_owned())]
        )
        .await
        .expect_err("must refuse")
        .code(),
        "unknown_channel"
    );
    assert_eq!(
        ops::dismiss(
            &server.state,
            READ_CHANNEL,
            &[MessageId("1000000000000009999".to_owned())]
        )
        .await
        .expect_err("must refuse")
        .code(),
        "unknown_message"
    );
    assert_eq!(
        ops::declare_bankruptcy(
            &server.state,
            READ_CHANNEL,
            &MessageId("1000000000000009999".to_owned())
        )
        .await
        .expect_err("must refuse")
        .code(),
        "unknown_message"
    );
    assert!(
        server
            .store
            .dismissals(&server.channel)
            .await
            .expect("reads")
            .is_empty(),
        "a refused request wrote to the store anyway"
    );
}

#[tokio::test]
async fn one_unknown_id_refuses_the_whole_batch_rather_than_dismissing_the_rest() {
    // Partial success is the worst answer available here: the caller is told it worked, its undo
    // describes a set that was never dismissed, and the difference is invisible until the reader
    // presses undo and part of the backlog stays gone.
    let server = Server::with(3);
    let error = ops::dismiss(
        &server.state,
        READ_CHANNEL,
        &[server.id(0), MessageId("1000000000000009999".to_owned())],
    )
    .await
    .expect_err("must refuse");
    assert_eq!(error.code(), "unknown_message");
    assert_eq!(
        server.left().await.len(),
        3,
        "a refused batch dismissed part of itself"
    );
}

#[tokio::test]
async fn an_undo_still_works_for_a_message_that_has_scrolled_out_of_the_window() {
    // Deliberately NOT symmetric with `dismiss`. An undo only ever removes a row, so there is
    // nothing to guard against — and refusing it because the message has since aged out of the
    // fetch window would make the undo fail exactly when the reader most needs it.
    let server = Server::with(2);
    server
        .store
        .dismiss(
            &server.channel,
            &[MessageId("1000000000000009999".to_owned())],
        )
        .await
        .expect("writes directly, as an older window once did");

    let change = ops::restore(
        &server.state,
        READ_CHANNEL,
        &[MessageId("1000000000000009999".to_owned())],
    )
    .await
    .expect("an undo must not need the message to still be fetchable");
    assert_eq!(change.messages.len(), 1);
    assert!(server
        .store
        .dismissals(&server.channel)
        .await
        .expect("reads")
        .is_empty());
}

#[tokio::test]
async fn a_to_do_list_with_no_store_refuses_rather_than_showing_the_whole_channel() {
    // The opposite posture from `summarize_message`, and the difference is the point. There the
    // store is a CACHE, so an absent one produces the same answer more slowly. Here the store IS
    // the answer: with no overlay every message is undealt-with, so a to-do list served from
    // nowhere is the plain channel wearing a name that promises filtering.
    let (state, discord) = testing::state_with(Arc::new(gent_talk::store::disabled::DisabledStore));
    discord.seed(&ChannelId(READ_CHANNEL.to_owned()), "codex-eng", "hello");
    let error = ops::todo(&state, READ_CHANNEL, None)
        .await
        .expect_err("must refuse");
    assert_eq!(error.code(), "storage_not_configured", "{error}");

    // THE CONTROL: the same channel, the same read, a store that works — and it answers.
    let (state, discord) = testing::state_with(Arc::new(FakeStore::new()));
    discord.seed(&ChannelId(READ_CHANNEL.to_owned()), "codex-eng", "hello");
    assert_eq!(
        ops::todo(&state, READ_CHANNEL, None)
            .await
            .expect("reads")
            .messages
            .len(),
        1
    );
}

#[tokio::test]
async fn the_overlay_is_bounded_and_the_oldest_marks_go_first() {
    // Every table in this store is bounded on write, and this one is no exception — a caller that
    // dismisses a different message every second must not be able to grow the file forever.
    let server = Server::with_retention(
        5,
        Retention {
            max_dismissals: 3,
            ..Retention::default()
        },
    );
    for n in 0..5 {
        server
            .store
            .dismiss(&server.channel, &[server.id(n)])
            .await
            .expect("dismisses");
        // A distinct millisecond per write, so "oldest first" is an ordering rather than a tie.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let held = server
        .store
        .dismissals(&server.channel)
        .await
        .expect("reads");
    assert_eq!(held.len(), 3, "the overlay is unbounded");
    assert_eq!(
        server.left().await,
        vec![server.ids[0].0.clone(), server.ids[1].0.clone()],
        "the ceiling evicted the wrong end: the marks the reader made most recently must survive"
    );
}

#[tokio::test]
async fn a_snowflake_that_cannot_be_ordered_is_refused_before_anything_is_written() {
    // Straight at the store, because `ops::dismiss` would refuse it earlier as unknown. The store
    // has to be able to say no on its own: the shipped one indexes by numeric position so that
    // "everything through here" is a range, and an id it cannot place has nowhere to go.
    let store = FakeStore::new();
    let channel = ChannelId(READ_CHANNEL.to_owned());
    let error = store
        .dismiss(
            &channel,
            &[
                MessageId("1000000000000000001".to_owned()),
                MessageId("not-a-snowflake".to_owned()),
            ],
        )
        .await
        .expect_err("must refuse");
    assert_eq!(error.code(), "bad_id", "{error}");
    assert!(
        store.dismissals(&channel).await.expect("reads").is_empty(),
        "the orderable half of a refused batch was written anyway"
    );
}
