//! The colour each thread is drawn in, decided here rather than in the browser. `#427
//! server-thread-hues`.
//!
//! Until this, the page chose thread colours itself (`web/voice.js` `threadHue`): `#396
//! thread-color-memory` remembered them per browser in localStorage, so two devices could colour
//! the same thread differently and there was no one answer to "which colour is that thread". The
//! owner, 2026-10-22: thread colours should be server side — whatever the policy is, it should be
//! decided as messages arrive on the server. So the server assigns each thread's hue ONCE, when
//! it first learns of the thread, stores it in [`crate::store`], and carries it on every message
//! thread record and thread summary it serves. A thread never changes colour once assigned (while
//! the assignment is retained; the store's bound and purge are stated at
//! [`crate::store::MAX_THREAD_HUES_PER_CHANNEL`]).
//!
//! The policy itself is `#414 thread-hue-spread`'s, ported from the page so the colours behave as
//! they always have, now with one decider:
//!
//! * a new thread takes the middle of the widest unused arc of the hue wheel among the channel's
//!   assigned colours (210, a blue, for the very first);
//! * unless that lands too near its neighbours — the threads up to [`NEIGHBOURS`] either side of
//!   it in root-message time within the channel, and the [`RECENT`] threads assigned most
//!   recently — in which case it is re-rolled deterministically: every whole degree is a
//!   candidate, farthest from every colour in use first and the lower hue on a tie, and the first
//!   that stands [`APART`] from every neighbour and [`BESIDE`] from the adjacent ones wins; where
//!   none does, the first that stands [`APART`] from every neighbour; where none does that either,
//!   the candidate farthest from its nearest neighbour.
//!
//! One divergence from the page, stated: the page's hues could be fractional (the middle of an
//! odd-width arc), and the server's are whole degrees. A hue is an index into a colour wheel at a
//! fixed saturation and lightness; half a degree is not a difference an eye can see at a 3px edge,
//! and whole degrees keep the stored value, the wire value and every comparison exact. A browser
//! hue seeded into the server (`#427`'s migration) is rounded to the nearest degree on the way in,
//! for the same reason.
//!
//! Determinism is load-bearing rather than tidy: two assignments raced on two connections must
//! not be able to disagree, which the store settles by deciding inside its write (see
//! [`crate::store::StateStore::ensure_thread_hues`]); and a channel re-learned after its hues
//! were purged is coloured the same way again when its threads arrive in the same order, because
//! nothing here reads a clock or a random source. Assignment order within one batch is
//! root-message time, then thread id — the order the threads stand in the channel — never the
//! order a caller's list happened to be in.

/// How far apart, in degrees of hue, a new thread's colour is kept from its neighbours'. 45° is
/// one step of the eight hue families the eye names at the page's fixed saturation and lightness,
/// so neighbours never share one; it is also as far as can still be promised, since eight
/// neighbours spread 45° apart leave nothing that clears them all. `#414 thread-hue-spread`.
pub const APART: u16 = 45;

/// How far, more strictly, from the threads right beside it, where it can be had: at 45° three
/// threads in a row can still step along the wheel and read as a gradient; a quarter turn reads
/// as unrelated. `#414 thread-hue-spread`.
pub const BESIDE: u16 = 90;

/// Threads either side of a new one, in root-message time, whose colours it avoids.
pub const NEIGHBOURS: usize = 3;

/// Threads assigned most recently whose colours a new one avoids too: the order threads arrived,
/// which is the order any view drew its new threads in.
pub const RECENT: usize = 3;

/// The first thread in a channel's colour, as on the page: a blue, the colour of a link, for the
/// thread a reader meets first.
pub const FIRST_HUE: u16 = 210;

/// One thread's assigned colour, as the store holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignedHue {
    /// The thread, in the channel-scoped opaque id the provider uses.
    pub thread_id: String,
    /// Its colour: whole degrees of hue, `0..360`.
    pub hue: u16,
    /// When its root message was posted, in Unix milliseconds, when this server has seen the
    /// root. What neighbours are ordered by. `None` for a thread first met through a reply whose
    /// root has not been read: such a thread takes part through the recent list alone, and its
    /// root time is filled in — its colour never revisited — when the root is read later.
    pub root_at_ms: Option<i64>,
    /// Where it stands in the order this channel's hues were assigned, from 1. What the recent
    /// list is read from, and the order the store's bound drops in.
    pub seq: i64,
}

/// A thread this server has just learnt of, and when its root was posted if that is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HueRef {
    /// The thread's channel-scoped opaque id.
    pub thread_id: String,
    /// Its root message's instant, in Unix milliseconds, when known.
    pub root_at_ms: Option<i64>,
}

/// The distance between two hues, the short way round the wheel.
#[must_use]
pub fn hue_apart(a: u16, b: u16) -> u16 {
    let apart = a.abs_diff(b) % 360;
    apart.min(360 - apart)
}

/// The middle of the widest arc `used` (sorted, each `0..360`) leaves unused, or [`FIRST_HUE`]
/// when nothing is used. The first widest arc wins, as on the page.
fn widest_middle(used: &[u16]) -> u16 {
    let mut hue = FIRST_HUE;
    let mut widest: i32 = -1;
    for (i, &at) in used.iter().enumerate() {
        let next = i32::from(*used.get(i + 1).unwrap_or(&used[0]))
            + if i + 1 < used.len() { 0 } else { 360 };
        let gap = next - i32::from(at);
        if gap > widest {
            widest = gap;
            hue = u16::try_from((i32::from(at) + gap / 2) % 360).unwrap_or(FIRST_HUE);
        }
    }
    hue
}

/// The least distance from `hue` to any colour in `of`; 360 when there are none.
fn clearance(hue: u16, of: &[u16]) -> u16 {
    of.iter()
        .fold(360, |least, &other| least.min(hue_apart(hue, other)))
}

/// `widest` if it clears the neighbours, else the deterministic re-roll. See the module docs.
fn spread(widest: u16, used: &[u16], near: &[u16], beside: &[u16]) -> u16 {
    let strict = |hue: u16| clearance(hue, near) >= APART && clearance(hue, beside) >= BESIDE;
    let plain = |hue: u16| clearance(hue, near) >= APART;
    if strict(widest) {
        return widest;
    }
    // Every whole degree, farthest from every colour in use first, the lower hue on a tie.
    // Clearance is computed once per candidate, as on the page: recomputing it inside the sort
    // would multiply the work by the comparison count, under the store's write lock.
    let mut candidates: Vec<(u16, u16)> = (0..360).map(|hue| (clearance(hue, used), hue)).collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let candidates: Vec<u16> = candidates.into_iter().map(|(_, hue)| hue).collect();
    for clears in [&strict as &dyn Fn(u16) -> bool, &plain] {
        if clears(widest) {
            return widest;
        }
        if let Some(first) = candidates.iter().copied().find(|hue| clears(*hue)) {
            return first;
        }
    }
    let mut best = candidates[0];
    for candidate in candidates {
        if clearance(candidate, near) > clearance(best, near) {
            best = candidate;
        }
    }
    best
}

/// The colours a new thread must stand clear of: `near`, up to [`NEIGHBOURS`] assigned threads
/// either side of it in root-message time plus the [`RECENT`] most recently assigned; and
/// `beside`, the nearest of those — the closest assigned root on either side, and the thread
/// assigned last. `assigned` is every hue handed out so far, this batch's included; `order` is
/// every known thread with a root time, in (time, id) order, assigned or not — one coloured
/// later avoids this one in turn, as on the page.
fn neighbours(target: &HueRef, order: &[HueRef], assigned: &[AssignedHue]) -> (Vec<u16>, Vec<u16>) {
    let hue_of = |id: &str| {
        assigned
            .iter()
            .find(|held| held.thread_id == id)
            .map(|held| held.hue)
    };
    let mut near = Vec::new();
    let mut beside = Vec::new();
    if let Some(at) = order
        .iter()
        .position(|held| held.thread_id == target.thread_id)
    {
        for step in [-1_isize, 1] {
            let mut found = 0;
            let mut i = at as isize + step;
            while i >= 0 && (i as usize) < order.len() && found < NEIGHBOURS {
                if let Some(hue) = hue_of(&order[i as usize].thread_id) {
                    near.push(hue);
                    if found == 0 {
                        beside.push(hue);
                    }
                    found += 1;
                }
                i += step;
            }
        }
    }
    let mut by_seq: Vec<&AssignedHue> = assigned.iter().collect();
    by_seq.sort_by_key(|held| held.seq);
    for held in by_seq.iter().rev().take(RECENT) {
        if held.thread_id != target.thread_id {
            near.push(held.hue);
        }
    }
    if let Some(last) = by_seq
        .last()
        .filter(|held| held.thread_id != target.thread_id)
    {
        beside.push(last.hue);
    }
    (near, beside)
}

/// Assign hues to every thread in `pending` that `existing` does not already colour, in
/// root-message time then thread id order, and return the new assignments with their `seq`
/// continuing `existing`'s. Pure: the same inputs always give the same colours, which is what
/// lets the store's backends share it and a test pin it.
///
/// A `pending` entry naming an already-coloured thread is not returned and never recolours it;
/// filling in such a thread's root time is the caller's separate, colour-free update.
#[must_use]
pub fn assign_new(existing: &[AssignedHue], pending: &[HueRef]) -> Vec<AssignedHue> {
    // The root-time order neighbours are read in: everything known with a root time, existing
    // rows keeping the time they were stored with where a pending ref repeats them without one.
    let mut order: Vec<HueRef> = Vec::new();
    for held in existing {
        if held.root_at_ms.is_some() {
            order.push(HueRef {
                thread_id: held.thread_id.clone(),
                root_at_ms: held.root_at_ms,
            });
        }
    }
    for wanted in pending {
        if wanted.root_at_ms.is_some()
            && !order.iter().any(|held| held.thread_id == wanted.thread_id)
        {
            order.push(wanted.clone());
        }
    }
    order.sort_by(|a, b| (a.root_at_ms, &a.thread_id).cmp(&(b.root_at_ms, &b.thread_id)));

    let mut todo: Vec<&HueRef> = pending
        .iter()
        .filter(|wanted| {
            !existing
                .iter()
                .any(|held| held.thread_id == wanted.thread_id)
        })
        .collect();
    // One assignment per thread however often a batch names it: the earliest root time given.
    // A thread whose root time is unknown goes LAST, not first: rooted threads take the arcs
    // their root order earns, and an unrooted one — a reply met before its root — avoids them
    // through the recent list, as it would arriving on its own a moment later.
    todo.sort_by(|a, b| {
        (a.root_at_ms.is_none(), a.root_at_ms, &a.thread_id).cmp(&(
            b.root_at_ms.is_none(),
            b.root_at_ms,
            &b.thread_id,
        ))
    });
    todo.dedup_by(|later, earlier| later.thread_id == earlier.thread_id);

    let mut assigned: Vec<AssignedHue> = existing.to_vec();
    let first_seq = existing.iter().map(|held| held.seq).max().unwrap_or(0) + 1;
    let mut fresh = Vec::new();
    for (next_seq, wanted) in (first_seq..).zip(todo) {
        let mut used: Vec<u16> = assigned.iter().map(|held| held.hue).collect();
        used.sort_unstable();
        let (near, beside) = neighbours(wanted, &order, &assigned);
        let hue = spread(widest_middle(&used), &used, &near, &beside);
        let row = AssignedHue {
            thread_id: wanted.thread_id.clone(),
            hue,
            root_at_ms: wanted.root_at_ms,
            seq: next_seq,
        };
        assigned.push(row.clone());
        fresh.push(row);
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(ids: &[&str]) -> Vec<HueRef> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| HueRef {
                thread_id: (*id).to_owned(),
                root_at_ms: Some(1_000_000 + i64::try_from(i).expect("small") * 60_000),
            })
            .collect()
    }

    #[test]
    fn the_first_thread_is_blue_and_each_new_one_splits_the_widest_arc() {
        let given = assign_new(&[], &refs(&["a", "b", "c"]));
        let hues: Vec<u16> = given.iter().map(|row| row.hue).collect();
        assert_eq!(hues, [210, 30, 120]);
    }

    #[test]
    fn assignment_is_deterministic_and_order_independent() {
        let one = assign_new(&[], &refs(&["a", "b", "c", "d", "e"]));
        // The same threads offered in another list order: root time decides, so nothing moves.
        let mut shuffled = refs(&["a", "b", "c", "d", "e"]);
        shuffled.reverse();
        let two = assign_new(&[], &shuffled);
        assert_eq!(one, two);
        // And again over the first result: nothing is recoloured or handed out twice.
        assert!(assign_new(&one, &refs(&["a", "b", "c", "d", "e"])).is_empty());
    }

    #[test]
    fn consecutive_threads_stand_clear_of_their_neighbours() {
        // The owner's case, server side: a long run of threads, each assigned as it arrives.
        let ids: Vec<String> = (0..24).map(|i| format!("t{i}")).collect();
        let given = assign_new(
            &[],
            &refs(&ids.iter().map(String::as_str).collect::<Vec<_>>()),
        );
        for (i, row) in given.iter().enumerate() {
            for other in given
                .iter()
                .skip(i.saturating_sub(NEIGHBOURS))
                .take(2 * NEIGHBOURS + 1)
            {
                if other.thread_id == row.thread_id {
                    continue;
                }
                // A thread can only avoid colours assigned before it; both directions hold
                // because each later thread avoided the earlier ones in turn.
                assert!(
                    hue_apart(row.hue, other.hue) >= APART,
                    "{} ({}) is {} from {} ({})",
                    row.thread_id,
                    row.hue,
                    hue_apart(row.hue, other.hue),
                    other.thread_id,
                    other.hue
                );
            }
        }
    }

    #[test]
    fn a_new_thread_between_two_assigned_ones_avoids_both() {
        // Learned out of order — a read that reaches an older thread last: its root time places
        // it between two already-coloured threads, and it must clear both.
        let existing = vec![
            AssignedHue {
                thread_id: "before".into(),
                hue: 200,
                root_at_ms: Some(100),
                seq: 1,
            },
            AssignedHue {
                thread_id: "after".into(),
                hue: 220,
                root_at_ms: Some(300),
                seq: 2,
            },
        ];
        let given = assign_new(
            &existing,
            &[HueRef {
                thread_id: "middle".into(),
                root_at_ms: Some(200),
            }],
        );
        assert_eq!(given.len(), 1);
        assert!(hue_apart(given[0].hue, 200) >= APART, "{}", given[0].hue);
        assert!(hue_apart(given[0].hue, 220) >= APART, "{}", given[0].hue);
        assert_eq!(given[0].seq, 3);
    }

    #[test]
    fn a_thread_without_a_root_time_is_coloured_through_the_recent_list() {
        let existing = vec![AssignedHue {
            thread_id: "known".into(),
            hue: 210,
            root_at_ms: Some(100),
            seq: 1,
        }];
        let given = assign_new(
            &existing,
            &[HueRef {
                thread_id: "reply-first".into(),
                root_at_ms: None,
            }],
        );
        assert_eq!(given.len(), 1);
        assert!(hue_apart(given[0].hue, 210) >= BESIDE, "{}", given[0].hue);
    }

    #[test]
    fn widest_middle_of_nothing_is_the_first_hue() {
        assert_eq!(widest_middle(&[]), FIRST_HUE);
        assert_eq!(widest_middle(&[210]), 30);
        assert_eq!(hue_apart(350, 10), 20);
        assert_eq!(hue_apart(0, 180), 180);
    }
}
