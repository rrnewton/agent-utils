//! The merge rules: what an incoming copy of a message does to the copy the cache holds.
//!
//! Pure decisions, applied by the writer. The rules, in order of precedence:
//!
//! 1. **Identity** is the channel and the provider's message id. Order is creation time, then the
//!    numeric id.
//! 2. **Content versions.** An incoming copy replaces the held content only when its hash differs
//!    AND either it was observed later than the held one or its provider edit time is later. A
//!    listing is observed when its read STARTED (for a snapshot that says how old it is, the
//!    earlier of the two), so an edit or deletion pushed while a read was running is never
//!    reverted by that read. A message once edited stays edited, and its edit time only moves
//!    later, so a copy whose edit time is older than the held copy's is of an earlier version and
//!    never replaces it, however late it was observed; and where every copy a provider gives
//!    reports the message's edits ([`Offered::reports_edits`]), a copy that reports none is of a
//!    version older than a held copy that reports one, and one that reports an edit is newer than
//!    a held copy that reports none. That is what keeps a copy a provider served from memory (a
//!    Slack reply Flat last read in full up to a quarter of an hour before, offered as seen at the
//!    window's start) from putting back text a later read had already replaced.
//! 3. **Deletions** delete the row and leave a [tombstone](crate::cache) holding nothing derived
//!    from the content. Only a read that started after the deletion brings the id back. A push
//!    never does: a pushed creation or edit that lands after the deletion (an event retried, two
//!    events racing, a poller's publish of what it read before) is of a message already gone.
//! 4. **Absence from an exhaustive listing** of a row's HOME scope deletes it, if it was observed
//!    before the listing started and lies below the scope's unsettled band. Absence from a listing
//!    of another scope only removes that membership. A message the cache does not hold yet is held
//!    to the same rule when it arrives: a copy observed before a listing of its home scope that
//!    proved, without it, the stretch it was written in is of a message gone by then, and leaves a
//!    tombstone instead of a row. So a read that began before a deletion never brings the message
//!    back by landing after a read that began after it and proved the stretch it was in, whether
//!    or not the cache held it before. The unsettled band, and anything above the head, is proved
//!    by nothing (a provider may list a message late, and a pushed copy may come before the
//!    listing does), so a copy there is held whatever a later listing lacked; but it is never
//!    vouched for unless the latest read of the scope's newest end listed it
//!    ([`super::shadow`]), and the listing that later settles its stretch takes it out.
//! 5. **Snippets never overwrite listing text.** A bridge falls back to a short snippet when a live
//!    event has no body, so a pushed update that is a strict prefix of the held text (a trailing
//!    `…` ignored) changes nothing, and a pushed create of an unknown id is held as partial until
//!    any listing replaces it.

/// How an incoming copy reached the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrival {
    /// A listing of one scope: a page, a window, a walk.
    Listing,
    /// Rows from a read that is not a listing of a scope: a delta, a posted message's echo.
    Rows,
    /// A pushed creation.
    PushCreate,
    /// A pushed edit.
    PushUpdate,
}

impl Arrival {
    /// Whether this came from the live feed rather than a read.
    #[must_use]
    pub fn is_push(self) -> bool {
        matches!(self, Self::PushCreate | Self::PushUpdate)
    }
}

/// The content the cache holds for a message.
#[derive(Clone, Copy, Debug)]
pub struct Held<'a> {
    /// The text.
    pub content: &'a str,
    /// Its hash.
    pub content_hash: &'a str,
    /// Local time of the observation that set it.
    pub observed_ms: i64,
    /// Provider edit time, when known.
    pub edited_ms: Option<i64>,
    /// Whether a push set it, and it may be a snippet.
    pub partial: bool,
}

/// The content an incoming copy carries.
#[derive(Clone, Copy, Debug)]
pub struct Offered<'a> {
    /// The text.
    pub content: &'a str,
    /// Its hash.
    pub content_hash: &'a str,
    /// Local time the observation started.
    pub observed_ms: i64,
    /// Provider edit time, when reported.
    pub edited_ms: Option<i64>,
    /// Whether every copy of the message its provider gives reports its edits, so that a copy
    /// without an edit time is of a message not edited when it was read: Slack's, and Discord's
    /// own API. A bridge's copies may come by paths that cannot say (a pushed event), so its are
    /// compared by edit time only where both copies carry one.
    pub reports_edits: bool,
    /// How it arrived.
    pub arrival: Arrival,
    /// Whether the sender marked it as possibly a snippet.
    pub partial: bool,
}

/// What to do with the held content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Keep the held content as it is.
    Keep,
    /// Same text, now confirmed by a listing: keep it, and it is no longer partial.
    Confirm,
    /// Take the offered content, marked partial or not.
    Replace {
        /// Whether the new content may be a snippet.
        partial: bool,
    },
}

/// Which of two copies of one message is the later version by their provider edit times, when
/// that can be told: `Less` when the offered copy is the earlier. See rule 2.
#[must_use]
pub fn edit_order(held: &Held<'_>, offered: &Offered<'_>) -> Option<std::cmp::Ordering> {
    match (offered.edited_ms, held.edited_ms) {
        (Some(offered_ms), Some(held_ms)) => Some(offered_ms.cmp(&held_ms)),
        (Some(_), None) if offered.reports_edits => Some(std::cmp::Ordering::Greater),
        (None, Some(_)) if offered.reports_edits => Some(std::cmp::Ordering::Less),
        _ => None,
    }
}

/// Rules 2 and 5 for a message the cache already holds.
#[must_use]
pub fn content_verdict(held: &Held<'_>, offered: &Offered<'_>) -> Verdict {
    let listing = offered.arrival == Arrival::Listing;
    if held.content_hash == offered.content_hash {
        return if held.partial && listing && !offered.partial {
            Verdict::Confirm
        } else {
            Verdict::Keep
        };
    }
    // A snippet never overwrites text a read brought.
    if offered.arrival.is_push()
        && !held.partial
        && (offered.partial || is_snippet_of(offered.content, held.content))
    {
        return Verdict::Keep;
    }
    let order = edit_order(held, offered);
    // An earlier version never replaces a later one, however late it was seen.
    if order == Some(std::cmp::Ordering::Less) {
        return Verdict::Keep;
    }
    // Any listing replaces what a push may have cut short.
    if held.partial && listing {
        return Verdict::Replace { partial: false };
    }
    let newer_observation = offered.observed_ms > held.observed_ms;
    let newer_edit = order == Some(std::cmp::Ordering::Greater);
    if newer_observation || newer_edit {
        Verdict::Replace {
            partial: offered.arrival.is_push() && offered.partial,
        }
    } else {
        Verdict::Keep
    }
}

/// Whether a message the cache does not hold yet is stored as partial. Every push of an unknown
/// id is, until the bridge says which pushes carry whole bodies: it may be a snippet, and the
/// next listing will replace it.
#[must_use]
pub fn new_row_partial(arrival: Arrival, offered_partial: bool) -> bool {
    arrival.is_push() || offered_partial
}

/// Whether `offered` is a snippet of `held`: a strict prefix of it, once a trailing ellipsis and
/// whitespace are ignored.
#[must_use]
pub fn is_snippet_of(offered: &str, held: &str) -> bool {
    let stem = offered
        .trim_end()
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim_end();
    !stem.is_empty() && stem.len() < held.len() && held.starts_with(stem)
}

/// Rule 3: whether a copy that arrived as `arrival`, observed at `observed_ms`, brings back an id
/// deleted at `deleted_ms`. Only a read that started strictly later does; a push never does.
#[must_use]
pub fn revives(arrival: Arrival, deleted_ms: i64, observed_ms: i64) -> bool {
    !arrival.is_push() && observed_ms > deleted_ms
}

/// Rule 4: whether a home-scope row missing from an exhaustive listing is gone. Only a row the
/// listing could have seen: observed before the listing started, and below the unsettled band.
/// The same test decides a copy of a message the cache does not hold, against the claim that holds
/// its creation time: `settled_below_ms` is then that claim's end, which never enters the band.
#[must_use]
pub fn absent_is_deleted(
    row_observed_ms: i64,
    listing_observed_ms: i64,
    created_ms: i64,
    settled_below_ms: i64,
) -> bool {
    row_observed_ms < listing_observed_ms && created_ms < settled_below_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(content: &str, observed_ms: i64, partial: bool) -> (String, Held<'_>) {
        let hash = crate::cache::rows::content_hash(content);
        (
            hash,
            Held {
                content,
                content_hash: "",
                observed_ms,
                edited_ms: None,
                partial,
            },
        )
    }

    fn verdict(held_text: &str, held_at: i64, held_partial: bool, offered: Offered<'_>) -> Verdict {
        let (hash, mut h) = held(held_text, held_at, held_partial);
        h.content_hash = &hash;
        content_verdict(&h, &offered)
    }

    fn offer(content: &str, observed_ms: i64, arrival: Arrival) -> (String, i64, Arrival) {
        (
            crate::cache::rows::content_hash(content),
            observed_ms,
            arrival,
        )
    }

    fn run(
        held_text: &str,
        held_at: i64,
        held_partial: bool,
        content: &str,
        at: i64,
        arrival: Arrival,
    ) -> Verdict {
        let (hash, observed_ms, arrival) = offer(content, at, arrival);
        verdict(
            held_text,
            held_at,
            held_partial,
            Offered {
                content,
                content_hash: &hash,
                observed_ms,
                edited_ms: None,
                reports_edits: false,
                arrival,
                partial: false,
            },
        )
    }

    #[test]
    fn a_later_observation_replaces_and_an_earlier_one_never_reverts() {
        assert_eq!(
            run("old", 10, false, "new", 20, Arrival::Listing),
            Verdict::Replace { partial: false }
        );
        // A read that started before the edit was pushed brings back the old text: kept out.
        assert_eq!(
            run("edited", 20, false, "old", 15, Arrival::Listing),
            Verdict::Keep
        );
        assert_eq!(
            run("same", 10, false, "same", 30, Arrival::Listing),
            Verdict::Keep
        );
    }

    #[test]
    fn a_later_provider_edit_time_replaces_even_from_an_older_observation() {
        let hash = crate::cache::rows::content_hash("v2");
        let (held_hash, mut h) = held("v1", 50, false);
        h.content_hash = &held_hash;
        h.edited_ms = Some(100);
        let offered = Offered {
            content: "v2",
            content_hash: &hash,
            observed_ms: 40,
            edited_ms: Some(200),
            reports_edits: false,
            arrival: Arrival::Listing,
            partial: false,
        };
        assert_eq!(
            content_verdict(&h, &offered),
            Verdict::Replace { partial: false }
        );
        let older_edit = Offered {
            edited_ms: Some(90),
            ..offered
        };
        assert_eq!(content_verdict(&h, &older_edit), Verdict::Keep);
    }

    /// A copy whose edit time is older than the held copy's is an earlier version, observed late:
    /// kept out. Where the provider's copies always report edits, one that reports none is older
    /// than a held copy that reports one, and one that reports one is newer than a held copy that
    /// reports none; elsewhere a missing edit time says nothing, and the observation decides.
    #[test]
    fn an_earlier_version_never_replaces_a_later_one_however_late_it_was_seen() {
        let (held_hash, mut h) = held("the reply as edited", 50, false);
        h.content_hash = &held_hash;
        h.edited_ms = Some(100);
        let stale = crate::cache::rows::content_hash("an earlier reply");
        let offered = |edited_ms: Option<i64>, reports_edits: bool, arrival: Arrival| Offered {
            content: "an earlier reply",
            content_hash: &stale,
            observed_ms: 90,
            edited_ms,
            reports_edits,
            arrival,
            partial: false,
        };
        for arrival in [Arrival::Listing, Arrival::Rows, Arrival::PushUpdate] {
            assert_eq!(
                content_verdict(&h, &offered(Some(60), false, arrival)),
                Verdict::Keep,
                "{arrival:?}: an older edit time"
            );
            assert_eq!(
                content_verdict(&h, &offered(None, true, arrival)),
                Verdict::Keep,
                "{arrival:?}: no edit, from a provider that always reports one"
            );
            assert_eq!(
                content_verdict(&h, &offered(None, false, arrival)),
                Verdict::Replace { partial: false },
                "{arrival:?}: no edit time from a path that cannot say; the later observation wins"
            );
        }
        // Held unedited: a copy reporting an edit is newer, however early it was seen, where the
        // provider always reports edits; elsewhere the observation decides.
        let (unedited_hash, mut unedited) = held("first words", 50, false);
        unedited.content_hash = &unedited_hash;
        let early = |reports_edits: bool| Offered {
            observed_ms: 10,
            edited_ms: Some(30),
            ..offered(Some(30), reports_edits, Arrival::Listing)
        };
        assert_eq!(
            content_verdict(&unedited, &early(true)),
            Verdict::Replace { partial: false }
        );
        assert_eq!(content_verdict(&unedited, &early(false)), Verdict::Keep);
        // An older edit time keeps out even a listing over a partial row.
        let (partial_hash, mut partial) = held("the reply as edited", 50, true);
        partial.content_hash = &partial_hash;
        partial.edited_ms = Some(100);
        assert_eq!(
            content_verdict(&partial, &offered(Some(60), false, Arrival::Listing)),
            Verdict::Keep
        );
    }

    #[test]
    fn a_pushed_snippet_never_overwrites_listing_text_and_a_listing_replaces_a_partial() {
        let text = "The full body of a long message";
        for snippet in [
            "The full body",
            "The full body…",
            "The full body ...",
            "The full body …",
        ] {
            assert_eq!(
                run(text, 10, false, snippet, 20, Arrival::PushUpdate),
                Verdict::Keep,
                "{snippet:?}"
            );
        }
        // A real edit pushed later still lands.
        assert_eq!(
            run(
                text,
                10,
                false,
                "Something else entirely",
                20,
                Arrival::PushUpdate
            ),
            Verdict::Replace { partial: false }
        );
        // A partial row is replaced by any listing, even one that started earlier.
        assert_eq!(
            run("The full", 30, true, text, 5, Arrival::Listing),
            Verdict::Replace { partial: false }
        );
        // ...and confirmed by one that carries the same text.
        assert_eq!(
            run(text, 30, true, text, 5, Arrival::Listing),
            Verdict::Confirm
        );
        assert!(new_row_partial(Arrival::PushCreate, false));
        assert!(new_row_partial(Arrival::PushUpdate, false));
        assert!(!new_row_partial(Arrival::Listing, false));
        assert!(!new_row_partial(Arrival::Rows, false));
    }

    #[test]
    fn snippets_are_strict_prefixes_and_an_empty_stem_is_not_one() {
        assert!(is_snippet_of("abc", "abcdef"));
        assert!(!is_snippet_of("abcdef", "abcdef"));
        assert!(!is_snippet_of("abd", "abcdef"));
        assert!(!is_snippet_of("…", "abcdef"));
        assert!(!is_snippet_of("", "abcdef"));
    }

    #[test]
    fn only_a_later_read_revives_and_only_settled_unseen_rows_are_absent() {
        for read in [Arrival::Listing, Arrival::Rows] {
            assert!(revives(read, 100, 101), "{read:?}");
            assert!(!revives(read, 100, 100), "{read:?}");
            assert!(!revives(read, 100, 50), "{read:?}");
        }
        // A push never does, however late it lands.
        for push in [Arrival::PushCreate, Arrival::PushUpdate] {
            assert!(!revives(push, 100, 101), "{push:?}");
            assert!(!revives(push, 100, i64::MAX), "{push:?}");
        }
        assert!(absent_is_deleted(10, 20, 500, 1_000));
        assert!(
            !absent_is_deleted(20, 20, 500, 1_000),
            "pushed during the read"
        );
        assert!(
            !absent_is_deleted(10, 20, 1_000, 1_000),
            "inside the unsettled band"
        );
    }
}
