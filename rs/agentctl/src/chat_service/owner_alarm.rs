//! Telling the owner, in the chat, when their messages are not reaching the agent (#720
//! agentctl-muse-stale-draft), for `agentctl chat run --owner-alarm-after DURATION`.
//!
//! A request whose prompt the bridge cannot type waits, and is tried again at every scan. The
//! scan logs it and lists it in `delivery-alarm.json`, but nobody reads either while it waits:
//! on 2026-11-01 eight of the owner's messages waited up to an hour behind a draft in a Muse
//! coordinator's editor while the agent kept looking busy. So once a request has waited
//! undelivered for the alarm's duration, the bridge posts a `[chat-bridge WARNING]` to the
//! chat saying how many wait, for how long, and the reason the last attempt gave; it says so
//! again every as long until they are delivered, and then says once that delivery resumed. A
//! request that waits only because its agent is busy ([`waiting_on_busy_agent`]) is no fault and
//! raises nothing.
//!
//! A request whose prompt was quarantined as a PROBABLE MISROUTE is never typed again by
//! itself, so its owner is told at once, with the start of the message and how to send it again.
//! Requests already in that state when the service starts were reported, if at all, by the run
//! before, so they are not reported again.
//!
//! The repeats back off (#830 chat-bridge-warning-backoff): on 2026-11-03 the owner was told the
//! same thing every 5 minutes for over 4 hours. One episode of held requests gets one top-level
//! warning; each repeat was a reply in its thread, after 1, 3, 6 and 12 times the alarm's
//! duration, then every 36 times it (5, 15, 30 and 60 minutes, then every 3 hours, by default).
//! The backoff starts again when the held requests or the kind of reason change, and a new
//! top-level warning comes only after delivery resumed and then failed again. The episode is
//! saved in `owner-alarm.json`, so a restarted bridge carries on in the same thread. `chat alarm
//! ack` silences the warning until the held requests or the reason change, and `chat alarm
//! snooze` until a time; neither touches the requests, which are still delivered when they can
//! be.
//!
//! The repeats coalesce in place (#981 chat-bridge-alarm-coalesce): an episode has at most one
//! visible warning. The first warning is posted at the top level and its message is saved with
//! the episode; each repeat rewrites that message with the current count, age, reason and fix,
//! and when delivery resumes the same message is rewritten to a short resolved line naming the
//! ET time. Rewriting is an edit where the transport supports it (Slack `chat.update`, through
//! the outbound helper's `update` action); where it cannot edit but can delete, the old warning
//! is deleted and the replacement posted; a transport that can do neither keeps the #830
//! behaviour of replies in the first warning's thread, and one without threads posts at the top
//! level. The resolved line is kept rather than deleted so the channel keeps a one-line record
//! that the episode happened and when it ended; deleting it would also race a reader who is
//! looking at, or replying to, the warning. The other `[chat-bridge WARNING]` posters were
//! checked for the same stacking: the capture-miss warnings in `reply_source` and the
//! stale-composer notice each post once per event, and the watcher's notices already replace
//! their unread predecessor, so this episode warning is the only repeating one.

use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::chat_runtime::{DeliveryEntry, RequestPhase};
use crate::forward_compat::KeptKeys;

/// The default `--owner-alarm-after`.
pub const DEFAULT_AFTER: Duration = Duration::from_secs(5 * 60);
/// The waits between one episode's warnings, as multiples of the alarm's duration: after the
/// first warning, after the second, and so on; the last repeats.
pub(crate) const BACKOFF: [u64; 5] = [1, 3, 6, 12, 36];
/// The saved episode, in the state directory.
pub(crate) const STATE_FILE: &str = "owner-alarm.json";
/// The operator's acknowledgement or snooze, in the state directory.
pub(crate) const SILENCE_FILE: &str = "owner-alarm-silence.json";
/// Schema of `owner-alarm.json`.
pub(crate) const STATE_SCHEMA: &str = "agentctl-chat-owner-alarm/v1";
/// Schema of `owner-alarm-silence.json`.
pub(crate) const SILENCE_SCHEMA: &str = "agentctl-chat-owner-alarm-silence/v1";
/// The start of every notice, so a reader can tell it from a message the agent wrote.
const WARNING: &str = "[chat-bridge WARNING] ";
/// Characters of a delivery error a notice quotes.
const REASON_CHARS: usize = 600;

/// Where a notice is posted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Place {
    /// At the top of the channel, on its own.
    TopLevel,
    /// At the top of the channel, as the first warning of the episode: the repeats replace it
    /// in place, so its message is given back with [`OwnerAlarm::root_posted`].
    EpisodeRoot,
    /// As a reply in this thread, the episode's: the fallback for a transport that can neither
    /// rewrite nor delete the episode's warning (#981 chat-bridge-alarm-coalesce).
    Thread(String),
    /// By rewriting this already-posted message in place (#981 chat-bridge-alarm-coalesce):
    /// the episode's one visible warning, whose thread a fallback reply goes to.
    Replace {
        message_id: String,
        thread_id: Option<String>,
    },
}

/// One notice and where to post it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Notice {
    pub(crate) text: String,
    pub(crate) place: Place,
}

/// One episode of held requests, from the first warning until delivery resumes, as
/// `owner-alarm.json` saves it.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Episode {
    /// The kind of reason and the held requests' keys: when it changes, the backoff starts again.
    pub(crate) fingerprint: String,
    /// How many requests are held.
    #[serde(default)]
    pub(crate) held: usize,
    /// The first warning's message, and the thread the repeats go to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) root_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) thread_id: Option<String>,
    pub(crate) started_at_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_post_millis: Option<u64>,
    /// Warnings posted since the backoff last started.
    #[serde(default)]
    pub(crate) step: usize,
    /// Warnings posted in the episode.
    #[serde(default)]
    pub(crate) posts: usize,
    /// When the next warning is due, unless silenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) next_post_millis: Option<u64>,
    #[serde(flatten)]
    pub(crate) unknown: KeptKeys,
}

/// `owner-alarm.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Saved {
    pub(crate) schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) episode: Option<Episode>,
    #[serde(flatten)]
    pub(crate) unknown: KeptKeys,
}

/// `owner-alarm-silence.json`, which `chat alarm ack` and `chat alarm snooze` write.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Silence {
    #[serde(default)]
    pub(crate) schema: String,
    /// The episode fingerprint acknowledged: its warnings stop until it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) acked_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) acked_at_millis: Option<u64>,
    /// No warning until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snoozed_until_millis: Option<u64>,
    #[serde(flatten)]
    pub(crate) unknown: KeptKeys,
}

impl Silence {
    /// Whether it silences the warnings of an episode with `fingerprint` at `now_millis`.
    pub(crate) fn silences(&self, fingerprint: &str, now_millis: u64) -> bool {
        self.snoozed_until_millis
            .is_some_and(|until| now_millis < until)
            || self.acked_fingerprint.as_deref() == Some(fingerprint)
    }
}

/// The owner alarm of one `chat run`.
#[derive(Debug)]
pub(crate) struct OwnerAlarm {
    after: Duration,
    /// The current episode of held requests, if any.
    episode: Option<Episode>,
    /// Whether the saved episode has been read back, once per run.
    pub(crate) restored: bool,
    /// Quarantined requests already known: told, or found when the service started.
    quarantines_known: BTreeSet<String>,
    /// Whether the first scan has been seen, which only learns the quarantines already there.
    seeded: bool,
}

/// The kind of `reason`, which the backoff follows rather than its exact words: the words
/// change with every attempt (a pane, a count of characters), the kind only when something
/// different holds the requests.
pub(crate) fn reason_kind(reason: &str, disagreement: bool) -> &'static str {
    if disagreement {
        "status-disagreement"
    } else if reason.contains("did not type") || reason.contains("last status=staged") {
        "foreign-draft"
    } else if reason.contains("unsent draft") {
        "unsent-draft"
    } else if reason.contains("refusing input") {
        "input-refused"
    } else if reason.contains("not found") || reason.contains("no pane") {
        "pane-missing"
    } else {
        "other"
    }
}

/// How long after the `step`-th warning of a backoff the next is due, for an alarm `after`.
fn backoff(after: u64, step: usize) -> u64 {
    after.saturating_mul(BACKOFF[step.saturating_sub(1).min(BACKOFF.len() - 1)])
}

/// Whether a request left waiting for `reason` waits only because its agent is busy, which is no
/// fault: a Claude Code coordinator without `--deliver-while-working` takes no prompt during a
/// turn, which can last most of an hour. Such a request raises no alarm; one refused because
/// something is in the agent's input box, or for any other reason, does. A request no attempt
/// has given a reason for has not been tried yet.
pub(crate) fn waiting_on_busy_agent(reason: Option<&str>) -> bool {
    reason.is_none_or(|reason| {
        reason.contains("last status=working") || reason.contains("coordinator is not ready")
    })
}

/// Whether `entry` was quarantined as a probable misroute.
fn misrouted(entry: &DeliveryEntry) -> bool {
    entry.phase == RequestPhase::DeliveryUncertain
        && entry
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("PROBABLE MISROUTE"))
}

fn minutes(millis: u64) -> u64 {
    millis / 60_000
}

fn cut(text: &str, limit: usize) -> String {
    let mut cut: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        cut.push('…');
    }
    cut
}

/// What the owner can do about requests held for `reason`, as a sentence ending in a space, or
/// nothing.
fn hint(agent: &str, reason: &str) -> String {
    // An idle Muse whose editor holds a draft reads `staged`; agentctl submits a draft it typed
    // itself before it gives up, so one still there is not its own.
    if reason.contains("did not type") || reason.contains("last status=staged") {
        format!(
            "Text that agentctl did not type is in {agent}'s input box, so it is left alone; \
             submit or clear it there, and delivery resumes. "
        )
    } else if reason.contains("unsent draft") {
        format!(
            "A message agentctl typed is stuck unsent in {agent}'s input box; press Enter there, \
             and delivery resumes. "
        )
    } else if reason.contains("refusing input") && reason.contains("--replace") {
        // #831 chat-dead-pin-reanchor: the harness was restarted in the same pane.
        format!(
            "{agent}'s pane runs a new process that agentctl will not type into until someone \
             confirms it is {agent}: look at the pane, then run `agentctl anchor {agent} \
             --replace`, and delivery resumes. "
        )
    } else {
        String::new()
    }
}

impl OwnerAlarm {
    /// An alarm for requests held `after` or longer.
    pub(crate) fn new(after: Duration) -> Self {
        Self {
            after,
            episode: None,
            restored: false,
            quarantines_known: BTreeSet::new(),
            seeded: false,
        }
    }

    /// Carry on with `episode`, the one a run before saved.
    pub(crate) fn restore(&mut self, episode: Option<Episode>) {
        self.restored = true;
        self.episode = episode;
    }

    /// The current episode, to save.
    pub(crate) fn episode(&self) -> Option<&Episode> {
        self.episode.as_ref()
    }

    /// The episode's first warning was posted as `message_id`, starting the thread `thread_id`.
    pub(crate) fn root_posted(&mut self, message_id: &str, thread_id: String) {
        if let Some(episode) = self.episode.as_mut() {
            episode.root_message_id = Some(message_id.to_owned());
            episode.thread_id = Some(thread_id);
        }
    }

    /// The notices due at `now_millis` for the request records `entries`, as a scan read them,
    /// about the agent called `agent`, in the order to post them. Each is counted as posted: one
    /// that fails to post is logged by the caller and not posted again (an alarm still repeats).
    ///
    /// `herdr_idle` is Herdr's own report for the pane: when it says idle or done, a request
    /// whose last attempt read `working` is not waiting on a busy agent but on a status
    /// disagreement (#734 chat-status-disagreement), so the busy-agent exemption does not
    /// apply and the notice says so. That disagreement is why the alarm never fired in the
    /// incident: every held request's reason contained `last status=working`.
    pub(crate) fn due(
        &mut self,
        entries: &[DeliveryEntry],
        agent: &str,
        now_millis: u64,
        herdr_idle: bool,
        silence: &Silence,
    ) -> Vec<Notice> {
        let mut notices = Vec::new();
        if !std::mem::replace(&mut self.seeded, true) {
            self.quarantines_known = entries
                .iter()
                .filter(|entry| misrouted(entry))
                .map(|entry| entry.key.clone())
                .collect();
        }
        for entry in entries.iter().filter(|entry| misrouted(entry)) {
            if !self.quarantines_known.insert(entry.key.clone()) {
                continue;
            }
            let age = minutes(now_millis.saturating_sub(entry.admitted_at_millis));
            notices.push(Notice {
                text: format!(
                    "{WARNING}Your message to {agent} sent {age} min ago (\"{}\") was quarantined \
                     as a PROBABLE MISROUTE: it may have reached another pane, or no agent at all, \
                     and it will not be sent again by itself. Why: {}. If {agent} does not act on \
                     it, send it again.",
                    entry.excerpt,
                    cut(entry.reason.as_deref().unwrap_or_default(), REASON_CHARS)
                ),
                place: Place::TopLevel,
            });
        }
        self.quarantines_known
            .retain(|key| entries.iter().any(|entry| entry.key == *key));
        let after = u64::try_from(self.after.as_millis()).unwrap_or(u64::MAX);
        let held: Vec<&DeliveryEntry> = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.phase,
                    RequestPhase::Pending | RequestPhase::Submitting
                )
            })
            .filter(|entry| now_millis.saturating_sub(entry.admitted_at_millis) >= after)
            .filter(|entry| herdr_idle || !waiting_on_busy_agent(entry.reason.as_deref()))
            .collect();
        if held.is_empty() {
            if let Some(episode) = self.episode.take() {
                if episode.posts > 0 {
                    let place = match (episode.root_message_id, episode.thread_id) {
                        (Some(message_id), thread_id) => Place::Replace {
                            message_id,
                            thread_id,
                        },
                        (None, Some(thread)) => Place::Thread(thread),
                        (None, None) => Place::TopLevel,
                    };
                    notices.push(Notice {
                        text: format!(
                            "[chat-bridge] Resolved {}: your messages to {agent} are being \
                             delivered again.",
                            crate::chat_trace::et_clock((now_millis / 1_000) as f64)
                        ),
                        place,
                    });
                }
            }
            return notices;
        }
        let reason = held
            .iter()
            .find_map(|entry| entry.reason.as_deref())
            .unwrap_or_default();
        let disagreement = herdr_idle && waiting_on_busy_agent(Some(reason));
        let mut keys: Vec<&str> = held.iter().map(|entry| entry.key.as_str()).collect();
        keys.sort_unstable();
        let fingerprint = format!("{}:{}", reason_kind(reason, disagreement), keys.join(","));
        let episode = self.episode.get_or_insert_with(|| Episode {
            fingerprint: fingerprint.clone(),
            started_at_millis: now_millis,
            ..Episode::default()
        });
        if episode.fingerprint != fingerprint {
            // Something different holds them, or different ones are held: start the backoff
            // again, in the same thread.
            episode.fingerprint.clone_from(&fingerprint);
            episode.step = 0;
            episode.next_post_millis = episode
                .last_post_millis
                .map(|at| at.saturating_add(backoff(after, 0)));
        }
        episode.held = held.len();
        if silence.silences(&fingerprint, now_millis)
            || episode.next_post_millis.is_some_and(|at| now_millis < at)
        {
            return notices;
        }
        episode.step += 1;
        episode.posts += 1;
        episode.last_post_millis = Some(now_millis);
        let wait = backoff(after, episode.step);
        episode.next_post_millis = Some(now_millis.saturating_add(wait));
        let oldest = held
            .iter()
            .map(|entry| now_millis.saturating_sub(entry.admitted_at_millis))
            .max()
            .unwrap_or_default();
        let reason = if disagreement {
            format!(
                "{reason} (status disagreement: Herdr reports the agent idle/done, \
                 agentctl's screen status says working)"
            )
        } else {
            reason.to_owned()
        };
        let reason = reason.as_str();
        let (count, verb) = if held.len() == 1 {
            ("1 of your messages".to_owned(), "has")
        } else {
            (format!("{} of your messages", held.len()), "have")
        };
        let place = match episode.root_message_id.clone() {
            Some(message_id) => Place::Replace {
                message_id,
                thread_id: episode.thread_id.clone(),
            },
            None => Place::EpisodeRoot,
        };
        let oldest_admitted = held
            .iter()
            .map(|entry| entry.admitted_at_millis)
            .min()
            .unwrap_or(now_millis);
        notices.push(Notice {
            text: format!(
                "{WARNING}{count} to {agent} {verb} not reached it, the oldest sent {} min \
                 ago ({}, \"{}\"). Why: {}. {}This notice is updated in place in {} min unless \
                 they are delivered; `agentctl chat alarm ack` silences it until the held messages \
                 or the reason change.",
                minutes(oldest),
                crate::chat_trace::et_clock((oldest_admitted / 1_000) as f64),
                held[0].excerpt,
                cut(reason, REASON_CHARS),
                hint(agent, reason),
                minutes(wait).max(1)
            ),
            place,
        });
        notices
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, phase: RequestPhase, admitted_minute: u64, reason: &str) -> DeliveryEntry {
        DeliveryEntry {
            key: key.to_owned(),
            phase,
            admitted_at_millis: admitted_minute * 60_000,
            delivered_at_millis: None,
            reason: (!reason.is_empty()).then(|| reason.to_owned()),
            excerpt: format!("message {key}"),
            settled: false,
        }
    }

    const DRAFT: &str = "request remains pending: pane wJ:pGY: the Muse editor holds a draft that \
                         agentctl did not type (41 characters on 1 row(s)); a person may be typing it";

    fn minute(m: u64) -> u64 {
        m * 60_000
    }

    /// The texts of `notices`.
    fn texts(notices: &[Notice]) -> Vec<&str> {
        notices.iter().map(|notice| notice.text.as_str()).collect()
    }

    /// `alarm.due` with no silence, the first warning of an episode posted as message `root`.
    fn due(alarm: &mut OwnerAlarm, entries: &[DeliveryEntry], at: u64) -> Vec<Notice> {
        let notices = alarm.due(entries, "coord5", at, false, &Silence::default());
        if notices
            .iter()
            .any(|notice| notice.place == Place::EpisodeRoot)
        {
            alarm.root_posted("C1/messages/1.5", "C1/threads/1.5".to_owned());
        }
        notices
    }

    #[test]
    fn a_restarted_harness_warning_names_the_anchor_command() {
        // #831 chat-dead-pin-reanchor: a dead pin with a live same-kind harness is never a
        // silent "not reached it"; the warning says why and what to run.
        let refused = "request remains pending: refusing input to agent 'coord5': the anchored \
                       claude process (pid 641842) has exited, and pane w16:pD now runs another \
                       claude process (pid 999); if it is coord5, run `agentctl anchor coord5 \
                       --replace` to pin it (look at the pane first: `agentctl attach coord5`), \
                       and delivery resumes; nothing was typed";
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let entries = vec![entry("a", RequestPhase::Pending, 100, refused)];
        let notices = due(&mut alarm, &entries, minute(105));
        assert_eq!(notices.len(), 1);
        let text = &notices[0].text;
        assert!(
            text.contains("now runs another claude process (pid 999)"),
            "{text}"
        );
        assert!(
            text.contains(
                "coord5's pane runs a new process that agentctl will not type into until someone \
                 confirms it is coord5: look at the pane, then run `agentctl anchor coord5 \
                 --replace`, and delivery resumes."
            ),
            "{text}"
        );
    }

    #[test]
    fn a_held_request_raises_the_alarm_which_replaces_its_warning_until_delivery_resumes() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let mut entries = vec![
            entry("a", RequestPhase::Pending, 100, DRAFT),
            entry("c", RequestPhase::Delivered, 90, ""),
        ];
        assert!(due(&mut alarm, &entries, minute(104)).is_empty());
        let notices = due(&mut alarm, &entries, minute(105));
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].place, Place::EpisodeRoot);
        assert_eq!(
            notices[0].text,
            "[chat-bridge WARNING] 1 of your messages to coord5 has not reached it, the oldest \
             sent 5 min ago (20:40 ET, \"message a\"). Why: request remains pending: pane wJ:pGY: \
             the Muse editor holds a draft that agentctl did not type (41 characters on 1 \
             row(s)); a person may be typing it. Text that agentctl did not type is in coord5's \
             input box, so it is left alone; submit or clear it there, and delivery resumes. \
             This notice is updated in place in 5 min unless they are delivered; `agentctl chat \
             alarm ack` silences it until the held messages or the reason change."
        );
        assert!(due(&mut alarm, &entries, minute(109)).is_empty());
        let again = due(&mut alarm, &entries, minute(110));
        assert_eq!(again.len(), 1, "the alarm repeats");
        // #981: a repeat rewrites the episode's one visible warning, it does not add a message.
        assert_eq!(
            again[0].place,
            Place::Replace {
                message_id: "C1/messages/1.5".to_owned(),
                thread_id: Some("C1/threads/1.5".to_owned()),
            }
        );
        assert!(again[0].text.starts_with(
            "[chat-bridge WARNING] 1 of your messages to coord5 has not reached it, the \
             oldest sent 10 min ago (20:40 ET"
        ));
        assert!(
            again[0].text.contains("updated in place in 15 min"),
            "{}",
            again[0].text
        );
        entries[0].phase = RequestPhase::Delivered;
        let resumed = due(&mut alarm, &entries, minute(111));
        assert_eq!(
            texts(&resumed),
            [
                "[chat-bridge] Resolved 20:51 ET: your messages to coord5 are being delivered \
              again."
            ]
        );
        assert_eq!(
            resumed[0].place,
            Place::Replace {
                message_id: "C1/messages/1.5".to_owned(),
                thread_id: Some("C1/threads/1.5".to_owned()),
            }
        );
        assert!(due(&mut alarm, &entries, minute(130)).is_empty());
        assert!(alarm.episode().is_none());
    }

    /// #830 chat-bridge-warning-backoff: four hours stuck give a handful of warnings, one of them
    /// at the top level, instead of one every 5 minutes.
    #[test]
    fn four_hours_stuck_post_few_warnings_and_one_at_the_top_level() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let entries = [entry("a", RequestPhase::Pending, 0, DRAFT)];
        let mut posted = Vec::new();
        // One scan a minute for four hours after the first warning.
        for at in 0..=245 {
            for notice in due(&mut alarm, &entries, minute(at)) {
                posted.push((at, notice.place));
            }
        }
        let tops = posted
            .iter()
            .filter(|(_, place)| *place == Place::EpisodeRoot)
            .count();
        assert_eq!(tops, 1, "{posted:?}");
        assert!(posted.len() <= 7, "{posted:?}");
        let minutes: Vec<u64> = posted.iter().map(|(at, _)| *at).collect();
        // 5, 15, 30 and 60 minutes apart; the next, 3 hours on, falls past the four hours.
        assert_eq!(minutes, [5, 10, 25, 55, 115], "{posted:?}");
    }

    #[test]
    fn a_change_of_held_requests_or_reason_restarts_the_backoff_on_the_same_message() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let mut entries = vec![entry("a", RequestPhase::Pending, 0, DRAFT)];
        for at in [5, 10, 25] {
            assert_eq!(
                due(&mut alarm, &entries, minute(at)).len(),
                1,
                "minute {at}"
            );
        }
        // Next due at 55, but a message held from minute 31 starts the backoff again: due 5 min
        // after the last warning, so at once, on the same message.
        entries.push(entry("b", RequestPhase::Pending, 26, DRAFT));
        assert!(due(&mut alarm, &entries, minute(30)).is_empty());
        let restarted = due(&mut alarm, &entries, minute(31));
        assert_eq!(restarted.len(), 1);
        assert_eq!(
            restarted[0].place,
            Place::Replace {
                message_id: "C1/messages/1.5".to_owned(),
                thread_id: Some("C1/threads/1.5".to_owned()),
            }
        );
        assert!(
            restarted[0].text.contains("in 5 min"),
            "{}",
            restarted[0].text
        );
        let next = due(&mut alarm, &entries, minute(36));
        assert!(next[0].text.contains("in 15 min"), "{}", next[0].text);
        // A different kind of reason restarts it too: due 5 min after the last warning, not 15.
        let unsent = "the message agentctl typed is an unsent draft";
        for entry in &mut entries {
            entry.reason = Some(unsent.to_owned());
        }
        assert!(due(&mut alarm, &entries, minute(40)).is_empty());
        assert!(due(&mut alarm, &entries, minute(41))[0]
            .text
            .contains("press Enter"));
        // Recovery, then a new failure: a new top-level warning.
        for entry in &mut entries {
            entry.phase = RequestPhase::Delivered;
        }
        assert_eq!(due(&mut alarm, &entries, minute(40)).len(), 1);
        let again = [entry("c", RequestPhase::Pending, 40, DRAFT)];
        let notices = due(&mut alarm, &again, minute(45));
        assert_eq!(notices[0].place, Place::EpisodeRoot);
    }

    #[test]
    fn ack_silences_the_held_set_until_it_changes_and_snooze_until_its_time() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let mut entries = vec![entry("a", RequestPhase::Pending, 0, DRAFT)];
        assert_eq!(due(&mut alarm, &entries, minute(5)).len(), 1);
        let fingerprint = alarm.episode().expect("episode").fingerprint.clone();
        let acked = Silence {
            acked_fingerprint: Some(fingerprint),
            ..Silence::default()
        };
        for at in 6..120 {
            assert!(alarm
                .due(&entries, "coord5", minute(at), false, &acked)
                .is_empty());
        }
        // The episode is kept, so a new held message warns again, on the same message.
        entries.push(entry("b", RequestPhase::Pending, 100, DRAFT));
        let notices = alarm.due(&entries, "coord5", minute(120), false, &acked);
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].place,
            Place::Replace {
                message_id: "C1/messages/1.5".to_owned(),
                thread_id: Some("C1/threads/1.5".to_owned()),
            }
        );
        let snoozed = Silence {
            snoozed_until_millis: Some(minute(300)),
            ..Silence::default()
        };
        for at in 121..300 {
            assert!(alarm
                .due(&entries, "coord5", minute(at), false, &snoozed)
                .is_empty());
        }
        assert_eq!(
            alarm
                .due(&entries, "coord5", minute(300), false, &snoozed)
                .len(),
            1
        );
    }

    #[test]
    fn a_request_waiting_only_for_a_busy_agent_raises_no_alarm() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let entries = [
            entry(
                "a",
                RequestPhase::Pending,
                0,
                "pane w1:p2 did not become idle/done within \
                   0s; last status=working",
            ),
            entry(
                "b",
                RequestPhase::Pending,
                0,
                "coordinator is not ready; request remains \
                   pending",
            ),
            entry("c", RequestPhase::Submitting, 0, ""),
        ];
        assert!(due(&mut alarm, &entries, minute(40)).is_empty());
        assert!(waiting_on_busy_agent(None));
        let staged = "pane w1:p2 did not become idle/done within 0s; last status=staged";
        assert!(!waiting_on_busy_agent(Some(staged)));
        assert!(hint("coord", staged).contains("submit or clear it there"));
    }

    // #734 chat-status-disagreement: the incident's held requests all read `last
    // status=working`, so the busy-agent exemption silenced the alarm for hours while
    // Herdr said `done`. When Herdr reports idle/done, that reason alarms, naming the
    // disagreement.
    #[test]
    fn a_working_reason_alarms_when_herdr_reports_idle_or_done() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let entries = [entry(
            "a",
            RequestPhase::Pending,
            0,
            "request remains pending: pane wJ:pGY did not become idle/done within 0s; \
             last status=working",
        )];
        let silence = Silence::default();
        assert!(alarm
            .due(&entries, "coord", minute(40), false, &silence)
            .is_empty());
        let notices = alarm.due(&entries, "coord", minute(40), true, &silence);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].text.contains("status disagreement"),
            "{notices:?}"
        );
        assert!(
            notices[0].text.contains("last status=working"),
            "{notices:?}"
        );
    }

    #[test]
    fn a_misroute_quarantine_is_told_once_and_one_found_at_start_is_not() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let misroute = "pane wJ:pGY: PROBABLE MISROUTE, quarantined: target shows it";
        let mut entries = vec![entry("old", RequestPhase::DeliveryUncertain, 1, misroute)];
        assert!(due(&mut alarm, &entries, minute(2)).is_empty());
        entries.push(entry("new", RequestPhase::DeliveryUncertain, 10, misroute));
        entries.push(entry(
            "maybe",
            RequestPhase::DeliveryUncertain,
            10,
            "possibly typed",
        ));
        let notices = due(&mut alarm, &entries, minute(12));
        assert_eq!(
            texts(&notices),
            ["[chat-bridge WARNING] Your message to coord5 sent 2 min ago (\"message new\") was \
              quarantined as a PROBABLE MISROUTE: it may have reached another pane, or no agent \
              at all, and it will not be sent again by itself. Why: pane wJ:pGY: PROBABLE \
              MISROUTE, quarantined: target shows it. If coord5 does not act on it, send it \
              again."]
        );
        assert_eq!(notices[0].place, Place::TopLevel);
        assert!(due(&mut alarm, &entries, minute(13)).is_empty());
    }
}
