//! Review round 1 probes (scratch only, not for commit).

use super::fixtures::{message, newest_proof, Fixture, T0};
use super::tests::{m, STEP};
use super::*;
use crate::live::{LiveKind, LiveMessage};

const SEC: i64 = 1_000;

fn decide_by_id(fx: &Fixture, id: &str, shape: observe::Shape, asked_ms: i64) -> (shadow::Decision, Option<String>) {
    let ask = shadow::Ask::Message {
        consumer: shadow::Consumer::MessageById,
        id: MessageId(id.to_owned()),
    };
    let query = shadow::query(&ask, shape, false).expect("a query");
    let view = fx.cache.shadow_view(&fx.chan, &query).served().expect("served");
    let facts = shadow::Facts {
        asked_ms,
        live_fed: false,
        fed_since_ms: Some(fx.cache.fed_since_ms()),
        dropped_ms: fx.cache.last_drop_ms(&fx.chan),
        barrier: true,
        feed: shadow::Feed::of(shape),
    };
    let content = view.rows.first().map(|r| r.message.content.clone());
    (shadow::decide(&ask, &view, &facts), content)
}

/// An unpolled Slack (or Discord-native) channel whose adapter pushes full events: a message
/// edited once (a read brought its edit time), then edited again, and the adapter's pushed
/// update, which carries no edit time (a path that cannot say), tells the cache the new text.
/// The cache must not keep the text it was told was edited away, and must not vouch for it.
#[test]
fn rr1_an_adapter_pushed_edit_without_an_edit_time_is_not_thrown_away() {
    for shape in [observe::Shape::Slack, observe::Shape::DiscordNative] {
        let fx = Fixture::new("rr1-push-edit");
        let first = T0 + 6 * STEP + SEC;
        fx.clock.set(first);
        let mut rows: Vec<crate::model::Message> = (1..=6).map(m).collect();
        rows[2].content = "first edit".to_owned();
        rows[2].edited_timestamp = Some(crate::clock::iso_from_ms(T0 + 5 * STEP));
        let id = rows[2].id.0.clone();
        let incoming: Vec<Incoming> = rows
            .iter()
            .cloned()
            .map(|msg| Incoming::new(msg, Scope::Main))
            .collect();
        fx.offer(
            &fx.chan,
            Observation::Listing(Listing {
                scope: Scope::Main,
                rows: incoming,
                observed_ms: first,
                proof: Some(newest_proof(ListStart::At(T0 + STEP))),
            }),
        );
        // The adapter pushes the second edit a minute later, with no edit time.
        let at = first + 60 * SEC;
        fx.clock.set(at);
        let mut edited = rows[2].clone();
        edited.content = "second edit".to_owned();
        edited.edited_timestamp = None;
        let live = LiveMessage {
            event_id: "e1".to_owned(),
            kind: LiveKind::Update,
            historical: false,
            message: edited,
            self_posted: false,
        };
        fx.offer(&fx.chan, observe::pushed(shape, &live, at));
        fx.clock.set(at + 5 * SEC);
        let (d, content) = decide_by_id(&fx, &id, shape, at + 5 * SEC);
        eprintln!("{shape:?}: by id {d:?}, holds {content:?}");
        assert!(
            !(d == shadow::Decision::Serve && content.as_deref() == Some("first edit")),
            "{shape:?}: served text the adapter said was edited away: {d:?} {content:?}"
        );
    }
}

/// Unused helper kept so the module compiles on both trees.
#[allow(dead_code)]
fn _unused() -> crate::model::Message {
    message("x", "a", "b", T0)
}
