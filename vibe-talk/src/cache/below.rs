//! What the cache's claims prove about the history below a page: whether a step back from a page
//! may be answered from the file, and whether anything is older at all. `#650
//! cache-first-older-reads`.
//!
//! Pure, as [`super::coverage`] is: rows and claims in, a decision out.
//!
//! # The scope's start
//!
//! A claim reaches provider time 0 only when a read proved the scope's start (a provider page whose
//! `has_more` is false, [`super::observe`]) and the channel has no age limit; an age limit stamps
//! the claim from the cutoff instead, and eviction raises it above what it took. So a claim chain
//! from 0 up to a read's oldest row says the file holds every message older than it, as of the
//! oldest claim used; and a read that asked for more rows than it found holds none older. Both
//! together say nothing older exists. A Slack thread's start is never claimed; there the thread's
//! root is its start, and a read of a thread that holds it has nothing older. (Only of a thread:
//! every top-level Slack message is a root, of a thread that may have no replies.)

use super::coverage::{covers, Cover, Interval};
use super::rows::CachedMessage;

/// Whether nothing of the scope is older than `rows`' oldest, `rows` being the scope's newest (or
/// newest below some point) messages, oldest first, and `more_held` whether the file holds more of
/// the scope below them: in a `thread` scope it is the thread's root, or the file holds nothing
/// older and the claims cover every instant from the scope's start up to it. False for no rows.
#[must_use]
pub fn reaches_start(
    coverage: &[Interval],
    rows: &[CachedMessage],
    more_held: bool,
    thread: bool,
) -> bool {
    let Some(oldest) = rows.first() else {
        return false;
    };
    (thread && is_root(oldest))
        || (!more_held && covers(coverage, 0, oldest.created_ms.saturating_add(1)).is_some())
}

/// A step back answered from the file: what it proved, and whether there is more below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proven {
    /// The claims the step rests on.
    pub cover: Cover,
    /// Whether anything of the scope is older than the step's oldest row.
    pub has_more: bool,
}

/// Whether `rows`, the scope's newest at most `limit` messages below a page's oldest (written at
/// `at_ms`), oldest first, are every message of the scope in their stretch, `more_held` saying
/// whether the file holds more below them. The claims must cover the stretch without a gap, from
/// the oldest row up to and including `at_ms`; and a step shorter than `limit` with nothing more
/// held must be covered from the scope's start, or older messages may be missing from the file.
/// Covered from the start with nothing more held, or holding a `thread` scope's root, the step has
/// nothing older. `None` when the claims do not prove it: the step reads upstream.
#[must_use]
pub fn proven(
    coverage: &[Interval],
    rows: &[CachedMessage],
    at_ms: i64,
    (limit, more_held): (usize, bool),
    thread: bool,
) -> Option<Proven> {
    let to_ms = at_ms.checked_add(1)?;
    let root = thread && rows.first().is_some_and(is_root);
    if !more_held && !root {
        if let Some(cover) = covers(coverage, 0, to_ms) {
            return Some(Proven {
                cover,
                has_more: false,
            });
        }
    }
    let oldest = rows.first()?;
    if !(more_held || root || rows.len() >= limit) {
        return None;
    }
    let cover = covers(coverage, oldest.created_ms, to_ms)?;
    Some(Proven {
        cover,
        has_more: !root,
    })
}

/// The longest suffix of `rows` (as [`proven`] takes them) the claims cover without a
/// gap up to and including `at_ms`, as `(first kept row, its cover)`: what a step back
/// may serve at once while the whole step stays unproven (`#2066
/// vibe-talk-page-freeze`). Rows older than the first gap are NOT in it — a message
/// could stand in that gap upstream, and serving past it would put a hole in the
/// page. `None` when not even the newest row is covered up to the page.
#[must_use]
pub fn covered_suffix(
    coverage: &[Interval],
    rows: &[CachedMessage],
    at_ms: i64,
) -> Option<(usize, Cover)> {
    let to_ms = at_ms.checked_add(1)?;
    (0..rows.len()).find_map(|first| {
        covers(coverage, rows[first].created_ms, to_ms).map(|cover| (first, cover))
    })
}

/// Whether `row` is a thread's root.
fn is_root(row: &CachedMessage) -> bool {
    row.message
        .thread
        .as_ref()
        .is_some_and(|thread| thread.is_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::coverage::Source;
    use crate::cache::fixtures;

    fn claim(t0_ms: i64, t1_ms: i64) -> Interval {
        Interval {
            t0_ms,
            t1_ms,
            listed_ms: 1_000,
            full_ms: None,
            complete: false,
            source: Source::Read,
        }
    }

    fn row(created_ms: i64) -> CachedMessage {
        CachedMessage {
            message: fixtures::message(&created_ms.to_string(), "a", "text", created_ms),
            created_ms,
            observed_ms: 1_000,
            content_partial: false,
            vouched_ms: None,
        }
    }

    fn root(created_ms: i64) -> CachedMessage {
        let mut row = row(created_ms);
        let mut thread = fixtures::reply("x", "T1", "text", created_ms)
            .thread
            .expect("a reply's thread");
        thread.is_root = true;
        row.message.thread = Some(thread);
        row
    }

    #[test]
    fn the_start_is_reached_only_with_nothing_more_held_covered_from_zero_or_a_root() {
        assert!(reaches_start(
            &[claim(0, 500)],
            &[row(100), row(200)],
            false,
            false
        ));
        assert!(
            !reaches_start(&[claim(0, 500)], &[row(100), row(200)], true, false),
            "the file holds older rows"
        );
        assert!(
            !reaches_start(&[claim(50, 500)], &[row(100)], false, false),
            "an age limit"
        );
        assert!(
            !reaches_start(&[claim(0, 40), claim(60, 500)], &[row(100)], false, false),
            "a gap"
        );
        assert!(reaches_start(&[], &[root(100), row(200)], true, true));
        assert!(
            !reaches_start(&[], &[root(100), row(200)], false, false),
            "a top-level message is a root too"
        );
        assert!(!reaches_start(&[claim(0, 500)], &[], false, false));
    }

    #[test]
    fn a_full_step_needs_its_own_stretch_and_says_whether_more_is_older() {
        let rows = [row(100), row(200)];
        let step = proven(&[claim(80, 400)], &rows, 300, (2, true), false).expect("covered");
        assert!(step.has_more);
        assert_eq!(step.cover.listed_ms, 1_000);
        let last = proven(&[claim(0, 400)], &rows, 300, (2, false), false).expect("covered");
        assert!(
            !last.has_more,
            "nothing more held, and covered from the start"
        );
        assert!(
            proven(&[claim(80, 400)], &rows, 300, (2, false), false)
                .expect("covered")
                .has_more,
            "an age limit: older messages may be upstream"
        );
        assert_eq!(
            proven(&[claim(150, 400)], &rows, 300, (2, true), false),
            None,
            "below the claim"
        );
        assert_eq!(
            proven(&[claim(80, 300)], &rows, 300, (2, true), false),
            None,
            "not up to the page"
        );
    }

    #[test]
    fn a_short_step_must_reach_the_start() {
        let rows = [row(100)];
        let short = proven(&[claim(0, 400)], &rows, 300, (5, false), false).expect("from zero");
        assert!(!short.has_more);
        assert_eq!(
            proven(&[claim(80, 400)], &rows, 300, (5, false), false),
            None,
            "older messages may be missing from the file"
        );
        assert!(
            !proven(&[claim(0, 400)], &[], 300, (5, false), false)
                .expect("empty")
                .has_more
        );
        assert_eq!(proven(&[claim(10, 400)], &[], 300, (5, false), false), None);
    }

    #[test]
    fn a_covered_suffix_stops_at_the_first_gap_and_needs_the_newest_row_covered() {
        let rows = [row(100), row(200), row(250)];
        // The whole stretch covered: the suffix is every row (proven's own case).
        let (first, cover) = covered_suffix(&[claim(80, 400)], &rows, 300).expect("covered");
        assert_eq!(first, 0);
        assert_eq!(cover.listed_ms, 1_000);
        // A gap between the oldest rows: only what is newer than it may be served —
        // a message could stand in the gap, and serving row 100 would skip it.
        let (first, _) =
            covered_suffix(&[claim(80, 150), claim(180, 400)], &rows, 300).expect("suffix");
        assert_eq!(first, 1, "the row older than the gap rode along");
        // The newest row itself uncovered up to the page: nothing may be served.
        assert_eq!(covered_suffix(&[claim(80, 240)], &rows, 300), None);
        assert_eq!(covered_suffix(&[claim(80, 400)], &[], 300), None);
    }

    #[test]
    fn a_step_that_holds_the_root_has_nothing_older() {
        let rows = [root(100), row(200)];
        let step = proven(&[claim(100, 400)], &rows, 300, (5, false), true).expect("from the root");
        assert!(!step.has_more);
        assert_eq!(
            proven(&[claim(100, 400)], &rows, 300, (5, false), false),
            None,
            "outside a thread a root is any top-level message"
        );
    }
}
