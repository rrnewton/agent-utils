//! Searching the server's own copy of its channels: `GET /api/v1/channels/{id}/search` and, for
//! every channel at once, `GET /api/v1/search`. `#605 server-side-search` (`#521
//! server-side-search`).
//!
//! The page searches what it has loaded; this searches what the message cache holds, which with
//! `#595 cache-full-history`'s full mode is each channel's whole history. The language and the
//! matching are [`crate::search_query`]'s, the same rules as the page's copy of them, so a message
//! either finds it or neither does.
//!
//! **Only what the cache may hand out.** A search hands rows out of the cache, so it keeps the
//! gates a served read keeps ([`crate::cache::Sink::search`]): the write scope (a read token is
//! never answered from the cache, as `ops::may_serve` says), serving on, the channel on the
//! cache's list, its provider's account checked against the file and not suspended, and nothing
//! in doubt. A channel that fails one is not searched, and its scope says why, by code; the answer
//! is still a 200, so the page can say exactly what was and was not searched. Rows not seen since
//! an offer for the channel was dropped, and texts only a push brought, are withheld.
//!
//! **What it does not promise: freshness.** A served page rests on a read of the newest end made
//! seconds ago; a search reaches back through stretches the server last listed hours or days ago
//! (`as_of_ms` says when the oldest of those listings began, and `#595 cache-full-history` re-reads
//! them in tiers). A message deleted upstream since its stretch was last listed can still be a hit;
//! Show in channel reads the channel itself, and the hit is a pointer, never the message's last
//! word.
//!
//! **How far back, in the cache's own terms.** Each channel's scope says how far back every
//! message the server holds was tested (`searched_back_to_ms`), the start of what the server's
//! coverage vouches it holds (`held_from_ms`, of top-level messages only on Slack's All, whose
//! replies no claim covers), whether the server holds the channel's whole history (its walk
//! reached the channel's beginning recently and its coverage is not cut at the age limit), and
//! whether that walk is still under way, so the line can say "still expanding". Where a search
//! stopped on its budget, `resume_below_ms` says where the next continues.
//!
//! **No upstream search.** Searching the chat services themselves is deferred (owner,
//! 2026-10-27): `upstream` is always `none`, and the page says so.
//!
//! A hit held only as a push brought it may be a snippet, so it is never handed out as the message
//! (`partial` counts the rows matched on such text); a reply whose own thread is in doubt is left
//! out (`withheld`).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::auth::Scope;
use crate::cache::search::{ChannelSearch, SearchAsk, SearchRefusal, SearchView};
use crate::contract::{SearchResponse, SearchScope};
use crate::model::{ChannelInfo, Message};
use crate::ops::OpError;
use crate::search_query::Day;
use crate::state::AppState;

/// Hits one answer carries when the caller names no limit.
pub const DEFAULT_LIMIT: usize = 50;
/// The most hits one answer carries.
pub const MAX_LIMIT: usize = 200;
/// About how many rows of one channel one answer tests before it stops and says where to resume.
pub const BUDGET: usize = 20_000;
/// The longest query accepted, in characters.
pub const MAX_QUERY_CHARS: usize = 1_000;

/// What a caller asks.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    /// The query, in the search language.
    pub q: String,
    /// The reader's IANA time zone, for `before:` and `after:`; the server's own when absent or
    /// unknown.
    pub tz: Option<String>,
    /// Only messages created before this instant, in milliseconds (exclusive): the page's loaded
    /// floor, or where an earlier answer said to resume.
    pub below_ms: Option<i64>,
    /// Hits wanted, clamped to [`MAX_LIMIT`].
    pub limit: Option<usize>,
}

/// Search `channels`, newest hit first.
///
/// # Errors
///
/// [`OpError::EmptyQuery`] for a query with no term, or one longer than [`MAX_QUERY_CHARS`].
pub async fn search(
    state: &AppState,
    caller: Scope,
    channels: Vec<ChannelInfo>,
    ask: &Ask,
) -> Result<SearchResponse, OpError> {
    if ask.q.chars().count() > MAX_QUERY_CHARS {
        return Err(OpError::EmptyQuery);
    }
    let query = Arc::new(crate::search_query::parse(&ask.q));
    if query.is_empty() {
        return Err(OpError::EmptyQuery);
    }
    let zone = ask
        .tz
        .as_deref()
        .and_then(|name| crate::clock::zone(name).ok())
        .unwrap_or_else(|| state.config.timezone.clone());
    let below_ms = ask.below_ms.unwrap_or(i64::MAX);
    let limit = ask.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let now_ms = crate::store::now_ms();
    // Every channel at once, each on the blocking pool: the file's readers serve them side by side.
    let searches = channels.iter().map(|channel| {
        let sink = Arc::clone(&state.message_cache);
        let query = Arc::clone(&query);
        let zone = zone.clone();
        let id = channel.id.clone();
        async move {
            if caller != Scope::Write {
                return Err(SearchRefusal::ReadScope);
            }
            tokio::task::spawn_blocking(move || {
                let day_start_ms = |day: Day| {
                    zone.day_start_ms(day.year, day.month, day.day)
                        .unwrap_or_else(|| day.start_ms_at_offset(0))
                };
                sink.search(
                    &id,
                    &SearchAsk {
                        query: &query,
                        below_ms,
                        limit,
                        budget: BUDGET,
                        day_start_ms: &day_start_ms,
                    },
                )
            })
            .await
            .unwrap_or(Err(SearchRefusal::NoCache))
        }
    });
    let searched = futures_util::future::join_all(searches).await;
    let mut scopes = Vec::with_capacity(channels.len());
    // Newest first across every channel: (created, channel index, message).
    let mut found: Vec<(i64, usize, Message)> = Vec::new();
    for (index, (channel, searched)) in channels.iter().zip(searched).enumerate() {
        scopes.push(scope_of(channel, &searched, below_ms, now_ms));
        if let Ok(searched) = searched {
            for hit in searched.outcome.hits {
                // As new as the read that put it in the cache, as a served page's copies are.
                let message = Message {
                    observed_ms: Some(hit.observed_ms),
                    ..hit.message
                };
                found.push((hit.created_ms, index, message));
            }
        }
    }
    let resume_below_ms = merge(&mut found, &mut scopes, limit);
    let mut by_channel: BTreeMap<usize, Vec<Message>> = BTreeMap::new();
    for (_, index, message) in found {
        by_channel.entry(index).or_default().push(message);
    }
    let mut hits = Vec::new();
    for (index, messages) in by_channel {
        hits.extend(prepare(state, &channels[index], messages).await);
    }
    hits.sort_by_key(|message| std::cmp::Reverse(message.created_ms()));
    Ok(SearchResponse {
        hits,
        scopes,
        resume_below_ms,
        upstream: "none",
        untrusted_content_notice: crate::untrusted::NOTICE,
    })
}

/// Cut the hits of every channel (newest first once sorted) to one answer, and say where the next
/// resumes; each scope is then clamped to it.
///
/// The answer holds exactly the hits from where it resumes up, where every channel tested every row:
/// the newest point a channel stopped on its budget, or the millisecond of the last hit the limit
/// keeps, every hit of which is kept, so that the next answer, below it, skips nothing and repeats
/// nothing. A channel that searched further back than that point says only that point: what lies
/// below it in that channel is the next answer's.
fn merge(
    found: &mut Vec<(i64, usize, Message)>,
    scopes: &mut [SearchScope],
    limit: usize,
) -> Option<i64> {
    found.sort_by_key(|found| std::cmp::Reverse(found.0));
    let mut resume_below_ms = scopes.iter().filter_map(|s| s.resume_below_ms).max();
    if found.len() > limit {
        let last_ms = found[limit - 1].0;
        resume_below_ms = Some(resume_below_ms.map_or(last_ms, |at| at.max(last_ms)));
    }
    if let Some(resume) = resume_below_ms {
        found.retain(|(created_ms, _, _)| *created_ms >= resume);
        for scope in scopes.iter_mut().filter(|scope| scope.state == "searched") {
            scope.resume_below_ms =
                Some(scope.resume_below_ms.map_or(resume, |own| own.max(resume)));
            scope.searched_back_to_ms = scope.searched_back_to_ms.map(|back| back.max(resume));
        }
    }
    resume_below_ms
}

/// Stamp, mark and render one channel's hits as a timeline page's messages are.
async fn prepare(state: &AppState, channel: &ChannelInfo, messages: Vec<Message>) -> Vec<Message> {
    let mut page = crate::threads::TimelinePage {
        messages,
        ..crate::threads::TimelinePage::default()
    };
    crate::ops::stamp(state, &mut page.messages);
    crate::noise::filter_for(state, &channel.id)
        .await
        .mark(&mut page.messages);
    crate::render::fill_page(&state.config, &state.headings, channel, &mut page);
    page.messages
}

/// What one channel's search says of itself.
fn scope_of(
    channel: &ChannelInfo,
    searched: &Result<ChannelSearch, SearchRefusal>,
    below_ms: i64,
    now_ms: i64,
) -> SearchScope {
    let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    match searched {
        Err(refusal) => SearchScope {
            channel: channel.clone(),
            state: "not_searched",
            reason: Some(refusal.code()),
            ..SearchScope::empty(channel.clone())
        },
        Ok(searched) => {
            let outcome = &searched.outcome;
            let frontier = searched.frontier.as_ref();
            let horizon = outcome.horizon;
            // The backfill's own reading of "complete": a walk that reached the beginning with
            // every read proving its stretch, begun recently enough (`backfill::channel_history`).
            let complete = frontier
                .and_then(|f| f.complete_started_ms)
                .is_some_and(|began| {
                    now_ms.saturating_sub(began) < crate::cache::REVALIDATE_WITHIN_MS
                });
            let whole = complete && horizon.is_some_and(|h| !h.at_age_limit);
            SearchScope {
                channel: channel.clone(),
                state: "searched",
                reason: None,
                examined: count(outcome.examined),
                searched_back_to_ms: outcome.searched_back_to_ms(below_ms),
                held_from_ms: horizon.map(|h| h.from_ms),
                as_of_ms: horizon.map(|h| h.as_of_ms),
                top_level_only: searched.view == SearchView::All,
                whole,
                expanding: frontier.is_some_and(|f| f.cursor.is_some()),
                capped: count(outcome.capped),
                partial: count(outcome.partial),
                withheld: count(outcome.withheld),
                resume_below_ms: outcome.resume_below_ms,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChannelId, MessageId};

    fn hit(created_ms: i64, channel: usize) -> (i64, usize, Message) {
        let mut message =
            crate::cache::fixtures::message(&format!("m{created_ms}"), "a", "x", created_ms);
        message.channel_id = ChannelId(format!("c{channel}"));
        (created_ms, channel, message)
    }

    fn scope(resume: Option<i64>, back: Option<i64>) -> SearchScope {
        SearchScope {
            state: "searched",
            resume_below_ms: resume,
            searched_back_to_ms: back,
            ..SearchScope::empty(crate::model::ChannelInfo {
                id: ChannelId("c".to_owned()),
                label: "c".to_owned(),
                writable: false,
                alias: None,
                added: false,
                provider: None,
                write_source: None,
            })
        }
    }

    fn ids(found: &[(i64, usize, Message)]) -> Vec<String> {
        found.iter().map(|(_, _, m)| m.id.0.clone()).collect()
    }

    #[test]
    fn channels_that_stop_at_different_points_answer_only_above_the_newest_stop() {
        // Channel 0 stopped on its budget at 500; channel 1 reached back to 100.
        let mut found = vec![hit(900, 0), hit(700, 1), hit(300, 1), hit(600, 0)];
        let mut scopes = [scope(Some(500), Some(500)), scope(None, Some(100))];
        let resume = merge(&mut found, &mut scopes, 50);
        assert_eq!(resume, Some(500));
        assert_eq!(
            ids(&found),
            ["m900", "m700", "m600"],
            "channel 1's 300 is the next answer's"
        );
        assert_eq!(
            scopes[1].searched_back_to_ms,
            Some(500),
            "channel 1 claimed past what it showed"
        );
        assert_eq!(
            scopes[1].resume_below_ms,
            Some(500),
            "channel 1 goes on from the same point"
        );
    }

    #[test]
    fn a_limit_cuts_on_a_whole_millisecond_and_clamps_every_channel() {
        let mut found = vec![hit(900, 0), hit(800, 1), hit(800, 0), hit(700, 1)];
        found[2].2.id = MessageId("m800b".to_owned());
        let mut scopes = [scope(None, Some(10)), scope(None, Some(10))];
        let resume = merge(&mut found, &mut scopes, 2);
        assert_eq!(resume, Some(800));
        assert_eq!(
            ids(&found),
            ["m900", "m800", "m800b"],
            "every hit of the cut millisecond is kept"
        );
        assert!(scopes
            .iter()
            .all(|s| s.searched_back_to_ms == Some(800) && s.resume_below_ms == Some(800)));
    }

    #[test]
    fn with_nothing_cut_every_channel_keeps_its_own_reach() {
        let mut found = vec![hit(900, 0), hit(100, 1)];
        let mut scopes = [scope(None, Some(50)), scope(None, None)];
        assert_eq!(merge(&mut found, &mut scopes, 50), None);
        assert_eq!(found.len(), 2);
        assert_eq!(scopes[0].searched_back_to_ms, Some(50));
        assert_eq!(scopes[1].resume_below_ms, None);
    }
}
