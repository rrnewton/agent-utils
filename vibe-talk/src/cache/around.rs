//! What the cache's claims prove about the messages around one message: how many of its older and
//! newer neighbours an `around` read may hand back as ONE stretch with no hole in it, and whether
//! anything may lie beyond each edge. `#2426 vibe-talk-sparse-loading-program`, slice A.
//!
//! Pure, as [`super::below`] is: rows and claims in, a decision out.
//!
//! # The stretch
//!
//! The older side is proven exactly as a step back from a page whose oldest message is the target
//! is ([`below::proven`], and failing that its covered suffix, [`below::covered_suffix`]): the
//! claims cover every instant from the oldest row served up to and including the target's own.
//! The newer side mirrors it: the longest run of newer rows the claims cover without a gap from
//! the target's instant up to and including the newest row's. Both sides include the target's
//! instant, so whatever is served lies in one covered stretch, and a row the claims do not reach
//! is never served beyond a hole a message could stand in. A side the claims prove nothing of
//! serves nothing, and says there may be more.
//!
//! # The edges
//!
//! Below, "more" is the step back's own answer: nothing more is older only when the claims reach
//! the scope's start (see [`super::below`]). Above, nothing more is newer only when every newer row
//! the copy holds is served and the stretch reaches the scope's head, the newest message the latest
//! read of its newest end listed: the claims cover the target's instant up to and including the
//! head's, or the newest row served is at or past it. That is "the server knows of nothing newer",
//! never "nothing newer exists": a message written since that read is the live feed's to bring.
//!
//! # The bottom edge and a step back
//!
//! The page walks on below an around answer with its `next_before`, which is the served backward
//! cursor of [`crate::timeline_served`]: below the oldest message's instant, or at it and not that
//! message, as [`super::reader`]'s step back reads. That rule keeps every OTHER message of the
//! oldest message's millisecond, because a page's oldest message need not be the first of its
//! millisecond in the copy's order: a repeat is harmless, a skip is not. So an answer that
//! ends below on a millisecond it holds more than one row of would see those rows again on the
//! step. It keeps only the newest of them instead, and says there is more below: the step then
//! hands back the rest of that millisecond, and nothing twice. The one exception is the target's
//! own millisecond, which the answer must keep: a step back from an answer whose every older row
//! shares the target's millisecond (more than `below` messages in one millisecond, or `below` zero)
//! may hand back again the rows of that millisecond it showed.

use super::below;
use super::coverage::{covers, Head, Interval};
use super::rows::CachedMessage;

/// What an `around` read may serve of the rows it read: `older[older_from..]`, the target, and
/// `newer[..newer_to]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Around {
    /// The first of the older rows served; `older.len()` when none is.
    pub older_from: usize,
    /// How many of the newer rows are served, from the oldest.
    pub newer_to: usize,
    /// Whether anything of the scope may be older than the oldest message served.
    pub more_below: bool,
    /// Whether anything of the scope may be newer than the newest message served.
    pub more_above: bool,
    /// The oldest `listed_ms` among the claims the answer rests on: what it is as of. `None` when
    /// it rests on none (the target alone, with more on both sides).
    pub listed_ms: Option<i64>,
}

/// What may be served around `target`, given the rows the scope holds beside it in its order,
/// `older` (oldest first, at most `below_limit` of them, `more_older` whether the copy holds more
/// below them) and `newer` (oldest first, `more_newer` whether it holds more above them), the
/// scope's claims, its head, and the latest drop of an offer for the channel.
///
/// A claim listed at or before `dropped_ms` proves nothing: the offer dropped may have been an edit
/// or a deletion in the stretch, as for a step back. A row whose text a push brought and no read
/// has proven whole is never handed out, and may not be left out of a stretch either: a side stops
/// short of the nearest such row, saying there is more.
#[must_use]
pub fn around(
    coverage: &[Interval],
    head: Option<&Head>,
    dropped_ms: Option<i64>,
    target: &CachedMessage,
    (older, below_limit, more_older): (&[CachedMessage], usize, bool),
    (newer, more_newer): (&[CachedMessage], bool),
) -> Around {
    let claims: Vec<Interval> = coverage
        .iter()
        .filter(|claim| dropped_ms.is_none_or(|dropped| claim.listed_ms > dropped))
        .copied()
        .collect();
    let t = target.created_ms;
    // The rows each side may consider: up to the nearest partial one, which stands for more.
    let skip = older
        .iter()
        .rposition(|row| row.content_partial)
        .map_or(0, |at| at + 1);
    let (older_whole, more_older) = (&older[skip..], more_older || skip > 0);
    let whole = newer
        .iter()
        .position(|row| row.content_partial)
        .unwrap_or(newer.len());
    let (newer_whole, more_newer) = (&newer[..whole], more_newer || whole < newer.len());

    // Below: a step back from the target, as a page's step back is proven.
    let (mut older_from, more_below, older_listed) =
        match below::proven(&claims, older_whole, t, (below_limit, more_older), false) {
            Some(step) => (skip, step.has_more, Some(step.cover.listed_ms)),
            None => match below::covered_suffix(&claims, older_whole, t) {
                Some((first, cover)) => (skip + first, true, Some(cover.listed_ms)),
                None => (older.len(), true, None),
            },
        };

    // Above: the longest run of newer rows covered from the target's instant up to each.
    let newer_to = newer_whole
        .iter()
        .take_while(|row| covers(&claims, t, row.created_ms.saturating_add(1)).is_some())
        .count();
    let newer_listed = newer_whole[..newer_to].last().and_then(|row| {
        covers(&claims, t, row.created_ms.saturating_add(1)).map(|cover| cover.listed_ms)
    });
    let edge_ms = newer_whole[..newer_to]
        .last()
        .map_or(t, |row| row.created_ms);
    let every_newer_served = newer_to == newer_whole.len() && !more_newer;
    let head_cover = head.and_then(|head| {
        if edge_ms >= head.head_ms {
            Some(None)
        } else {
            covers(&claims, t, head.head_ms.saturating_add(1)).map(|cover| Some(cover.listed_ms))
        }
    });
    let more_above = !(every_newer_served && head_cover.is_some());

    // The bottom edge: never a millisecond of several served rows below the target's own, so a
    // step back from the answer repeats none of them (see the module documentation).
    if more_below {
        if let Some(oldest) = older.get(older_from) {
            if oldest.created_ms != t {
                older_from += older[older_from..]
                    .iter()
                    .take_while(|row| row.created_ms == oldest.created_ms)
                    .count()
                    - 1;
            }
        }
    }

    let listed_ms = [older_listed, newer_listed, head_cover.flatten()]
        .into_iter()
        .flatten()
        .min();
    Around {
        older_from,
        newer_to,
        more_below,
        more_above,
        listed_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::coverage::Source;
    use crate::cache::fixtures;

    fn claim(t0_ms: i64, t1_ms: i64) -> Interval {
        listed(t0_ms, t1_ms, 1_000)
    }

    fn listed(t0_ms: i64, t1_ms: i64, listed_ms: i64) -> Interval {
        Interval {
            t0_ms,
            t1_ms,
            listed_ms,
            full_ms: None,
            complete: false,
            source: Source::Read,
        }
    }

    fn row(id: &str, created_ms: i64) -> CachedMessage {
        CachedMessage {
            message: fixtures::message(id, "a", "text", created_ms),
            created_ms,
            observed_ms: 1_000,
            content_partial: false,
            vouched_ms: None,
        }
    }

    fn partial(id: &str, created_ms: i64) -> CachedMessage {
        CachedMessage {
            content_partial: true,
            ..row(id, created_ms)
        }
    }

    fn head(head_ms: i64) -> Head {
        Head {
            head_ms,
            unsettled_from_ms: head_ms + 1,
            head_listed_ms: 1_000,
        }
    }

    /// Rows at `100 * n` for each `n`, named `m<n>`.
    fn rows(range: std::ops::Range<i64>) -> Vec<CachedMessage> {
        range.map(|n| row(&format!("m{n}"), n * 100)).collect()
    }

    #[test]
    fn a_target_in_a_covered_stretch_serves_both_sides_whole_and_says_more_beyond_each() {
        // m0..m20 held and covered from 0 to past the head; around m10, three each way, with more
        // held on both sides.
        let all = rows(0..21);
        let claims = [claim(0, 2_100)];
        let proof = around(
            &claims,
            Some(&head(2_000)),
            None,
            &all[10],
            (&all[7..10], 3, true),
            (&all[11..14], true),
        );
        assert_eq!(
            proof,
            Around {
                older_from: 0,
                newer_to: 3,
                more_below: true,
                more_above: true,
                listed_ms: Some(1_000),
            }
        );
    }

    #[test]
    fn at_the_scopes_start_and_at_its_head_there_is_nothing_more() {
        let all = rows(0..6);
        let claims = [claim(0, 600)];
        // The oldest: nothing older held, and the claims reach the scope's start.
        let oldest = around(
            &claims,
            Some(&head(500)),
            None,
            &all[0],
            (&[], 3, false),
            (&all[1..4], true),
        );
        assert!(!oldest.more_below, "{oldest:?}");
        assert!(oldest.more_above, "{oldest:?}");
        // The newest: every newer row served, and the stretch reaches the head.
        let newest = around(
            &claims,
            Some(&head(500)),
            None,
            &all[4],
            (&all[1..4], 3, true),
            (&all[5..6], false),
        );
        assert_eq!(newest.newer_to, 1);
        assert!(!newest.more_above, "{newest:?}");
        assert!(newest.more_below, "{newest:?}");
        // The head itself as the target, nothing newer held.
        let at_head = around(
            &claims,
            Some(&head(500)),
            None,
            &all[5],
            (&all[2..5], 3, true),
            (&[], false),
        );
        assert!(!at_head.more_above, "{at_head:?}");
    }

    #[test]
    fn an_age_limit_or_no_head_leaves_more_on_that_side() {
        let all = rows(0..6);
        // Claims from 50 only: older messages may be upstream, below the limit.
        let clipped = around(
            &[claim(50, 600)],
            Some(&head(500)),
            None,
            &all[2],
            (&all[1..2], 3, false),
            (&all[3..6], false),
        );
        assert!(clipped.more_below, "{clipped:?}");
        assert_eq!(clipped.older_from, 0, "the covered row is still served");
        assert!(!clipped.more_above, "{clipped:?}");
        // No read of the newest end recorded: nothing says the stretch reaches it.
        let headless = around(
            &[claim(0, 600)],
            None,
            None,
            &all[2],
            (&all[0..2], 3, false),
            (&all[3..6], false),
        );
        assert!(!headless.more_below, "{headless:?}");
        assert!(headless.more_above, "{headless:?}");
        // The head beyond the claims (its band unsettled): the stretch cannot be said to reach it.
        let unsettled = around(
            &[claim(0, 450)],
            Some(&head(500)),
            None,
            &all[2],
            (&all[0..2], 3, false),
            (&all[3..5], false),
        );
        assert_eq!(unsettled.newer_to, 2);
        assert!(unsettled.more_above, "{unsettled:?}");
    }

    #[test]
    fn a_hole_on_either_side_truncates_that_side_and_says_more() {
        let all = rows(0..21);
        // A hole between m6 and m8 below, and between m12 and m14 above.
        let claims = [claim(0, 650), claim(750, 1_250), claim(1_350, 2_100)];
        let proof = around(
            &claims,
            Some(&head(2_000)),
            None,
            &all[10],
            (&all[5..10], 5, true),
            (&all[11..16], true),
        );
        // Older rows m5..m9: only m8 and m9 lie above the hole.
        assert_eq!(proof.older_from, 3, "{proof:?}");
        assert!(proof.more_below);
        // Newer rows m11..m15: only m11 and m12 lie below the hole.
        assert_eq!(proof.newer_to, 2, "{proof:?}");
        assert!(proof.more_above);
        // The target's own instant uncovered: neither side serves anything.
        let bare = around(
            &[claim(0, 1_000), claim(1_001, 2_100)],
            Some(&head(2_000)),
            None,
            &all[10],
            (&all[7..10], 3, true),
            (&all[11..14], true),
        );
        assert_eq!((bare.older_from, bare.newer_to), (3, 0), "{bare:?}");
        assert!(bare.more_below && bare.more_above);
        assert_eq!(bare.listed_ms, None);
    }

    #[test]
    fn a_short_older_side_must_reach_the_start_or_says_more() {
        let all = rows(0..6);
        // Two older rows held, three asked, nothing more held: covered from 100 only.
        let proof = around(
            &[claim(100, 600)],
            Some(&head(500)),
            None,
            &all[3],
            (&all[1..3], 3, false),
            (&all[4..6], false),
        );
        assert_eq!(
            proof.older_from, 0,
            "both rows are covered up to the target"
        );
        assert!(
            proof.more_below,
            "older messages may be missing from the copy"
        );
    }

    #[test]
    fn claims_listed_before_a_drop_prove_nothing() {
        let all = rows(0..11);
        let claims = [listed(0, 500, 1_000), listed(500, 1_100, 3_000)];
        let proof = around(
            &claims,
            Some(&head(1_000)),
            Some(2_000),
            &all[5],
            (&all[2..5], 3, true),
            (&all[6..9], true),
        );
        assert_eq!(
            proof.older_from, 3,
            "the older side rests on the dropped claim"
        );
        assert_eq!(proof.newer_to, 3, "the newer side rests on the later one");
        assert_eq!(proof.listed_ms, Some(3_000));
    }

    #[test]
    fn a_partial_row_ends_its_side_short_of_it() {
        let mut all = rows(0..11);
        all[3] = partial("m3", 300);
        all[8] = partial("m8", 800);
        let claims = [claim(0, 1_100)];
        let proof = around(
            &claims,
            Some(&head(1_000)),
            None,
            &all[5],
            (&all[1..5], 4, false),
            (&all[6..11], false),
        );
        // Older m1..m4: m3 is partial, so only m4 is served.
        assert_eq!(proof.older_from, 3, "{proof:?}");
        assert!(proof.more_below);
        // Newer m6..m10: m8 is partial, so m6 and m7.
        assert_eq!(proof.newer_to, 2, "{proof:?}");
        assert!(proof.more_above);
    }

    #[test]
    fn the_bottom_edge_keeps_only_the_newest_row_of_a_shared_millisecond() {
        // a, b and c share instant 200 below the target at 400; more is held below a.
        let older = [row("a", 200), row("b", 200), row("c", 200), row("d", 300)];
        let target = row("t", 400);
        let claims = [claim(0, 1_000)];
        let proof = around(
            &claims,
            Some(&head(900)),
            None,
            &target,
            (&older, 4, true),
            (&[], false),
        );
        assert_eq!(proof.older_from, 2, "c, the newest of instant 200, then d");
        assert!(proof.more_below);
        // Nothing more below: no step back follows, so nothing is trimmed.
        let whole = around(
            &claims,
            Some(&head(900)),
            None,
            &target,
            (&older, 5, false),
            (&[], false),
        );
        assert_eq!(whole.older_from, 0);
        assert!(!whole.more_below);
        // The target's own millisecond is kept whole: the target must be served.
        let same = [row("a", 400), row("b", 400)];
        let tie = around(
            &claims,
            Some(&head(900)),
            None,
            &target,
            (&same, 2, true),
            (&[], false),
        );
        assert_eq!(tie.older_from, 0);
    }
}
