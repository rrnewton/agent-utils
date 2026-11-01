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

use std::collections::BTreeSet;
use std::time::Duration;

use crate::chat_runtime::{DeliveryEntry, RequestPhase};

/// The default `--owner-alarm-after`.
pub const DEFAULT_AFTER: Duration = Duration::from_secs(5 * 60);
/// The start of every notice, so a reader can tell it from a message the agent wrote.
const WARNING: &str = "[chat-bridge WARNING] ";
/// Characters of a delivery error a notice quotes.
const REASON_CHARS: usize = 600;

/// The owner alarm of one `chat run`.
#[derive(Debug)]
pub(crate) struct OwnerAlarm {
    after: Duration,
    /// When the alarm was last posted for the current episode of held requests.
    last_alarm_millis: Option<u64>,
    /// Quarantined requests already known: told, or found when the service started.
    quarantines_known: BTreeSet<String>,
    /// Whether the first scan has been seen, which only learns the quarantines already there.
    seeded: bool,
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
    } else {
        String::new()
    }
}

impl OwnerAlarm {
    /// An alarm for requests held `after` or longer.
    pub(crate) fn new(after: Duration) -> Self {
        Self {
            after,
            last_alarm_millis: None,
            quarantines_known: BTreeSet::new(),
            seeded: false,
        }
    }

    /// The notices due at `now_millis` for the request records `entries`, as a scan read them,
    /// about the agent called `agent`, in the order to post them. Each is counted as posted: one
    /// that fails to post is logged by the caller and not posted again (an alarm still repeats).
    pub(crate) fn due(
        &mut self,
        entries: &[DeliveryEntry],
        agent: &str,
        now_millis: u64,
    ) -> Vec<String> {
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
            notices.push(format!(
                "{WARNING}Your message to {agent} sent {age} min ago (\"{}\") was quarantined as \
                 a PROBABLE MISROUTE: it may have reached another pane, or no agent at all, and \
                 it will not be sent again by itself. Why: {}. If {agent} does not act on it, \
                 send it again.",
                entry.excerpt,
                cut(entry.reason.as_deref().unwrap_or_default(), REASON_CHARS)
            ));
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
            .filter(|entry| !waiting_on_busy_agent(entry.reason.as_deref()))
            .collect();
        if held.is_empty() {
            if self.last_alarm_millis.take().is_some() {
                notices.push(format!(
                    "[chat-bridge] Your messages to {agent} are being delivered again."
                ));
            }
            return notices;
        }
        if self
            .last_alarm_millis
            .is_some_and(|at| now_millis.saturating_sub(at) < after)
        {
            return notices;
        }
        self.last_alarm_millis = Some(now_millis);
        let oldest = held
            .iter()
            .map(|entry| now_millis.saturating_sub(entry.admitted_at_millis))
            .max()
            .unwrap_or_default();
        let reason = held
            .iter()
            .find_map(|entry| entry.reason.as_deref())
            .unwrap_or_default();
        let (count, verb) = if held.len() == 1 {
            ("1 of your messages".to_owned(), "has")
        } else {
            (format!("{} of your messages", held.len()), "have")
        };
        notices.push(format!(
            "{WARNING}{count} to {agent} {verb} not reached it, the oldest sent {} min ago \
             (\"{}\"). Why: {}. {}This warning repeats every {} min until they are delivered.",
            minutes(oldest),
            held[0].excerpt,
            cut(reason, REASON_CHARS),
            hint(agent, reason),
            minutes(after).max(1)
        ));
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
        }
    }

    const DRAFT: &str = "request remains pending: pane wJ:pGY: the Muse editor holds a draft that \
                         agentctl did not type (41 characters on 1 row(s)); a person may be typing it";

    #[test]
    fn a_held_request_raises_the_alarm_which_repeats_until_delivery_resumes() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let minute = |m: u64| m * 60_000;
        let mut entries = vec![
            entry("a", RequestPhase::Pending, 100, DRAFT),
            entry("b", RequestPhase::Submitting, 102, DRAFT),
            entry("c", RequestPhase::Delivered, 90, ""),
        ];
        assert!(alarm.due(&entries, "coord5", minute(104)).is_empty());
        let notices = alarm.due(&entries, "coord5", minute(105));
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0],
            "[chat-bridge WARNING] 1 of your messages to coord5 has not reached it, the oldest \
             sent 5 min ago (\"message a\"). Why: request remains pending: pane wJ:pGY: the Muse \
             editor holds a draft that agentctl did not type (41 characters on 1 row(s)); a \
             person may be typing it. Text that agentctl did not type is in coord5's input box, \
             so it is left alone; submit or clear it there, and delivery resumes. This warning \
             repeats every 5 min until they are delivered."
        );
        assert!(alarm.due(&entries, "coord5", minute(109)).is_empty());
        let again = alarm.due(&entries, "coord5", minute(110));
        assert_eq!(again.len(), 1, "the alarm repeats");
        assert!(again[0].contains(
            "2 of your messages to coord5 have not reached it, the oldest \
                                   sent 10 min ago"
        ));
        for entry in &mut entries[..2] {
            entry.phase = RequestPhase::Delivered;
        }
        assert_eq!(
            alarm.due(&entries, "coord5", minute(111)),
            ["[chat-bridge] Your messages to coord5 are being delivered again."]
        );
        assert!(alarm.due(&entries, "coord5", minute(130)).is_empty());
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
        assert!(alarm.due(&entries, "coord", 40 * 60_000).is_empty());
        assert!(waiting_on_busy_agent(None));
        let staged = "pane w1:p2 did not become idle/done within 0s; last status=staged";
        assert!(!waiting_on_busy_agent(Some(staged)));
        assert!(hint("coord", staged).contains("submit or clear it there"));
    }

    #[test]
    fn a_misroute_quarantine_is_told_once_and_one_found_at_start_is_not() {
        let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
        let misroute = "pane wJ:pGY: PROBABLE MISROUTE, quarantined: target shows it";
        let mut entries = vec![entry("old", RequestPhase::DeliveryUncertain, 1, misroute)];
        assert!(alarm.due(&entries, "coord5", 2 * 60_000).is_empty());
        entries.push(entry("new", RequestPhase::DeliveryUncertain, 10, misroute));
        entries.push(entry(
            "maybe",
            RequestPhase::DeliveryUncertain,
            10,
            "possibly typed",
        ));
        let notices = alarm.due(&entries, "coord5", 12 * 60_000);
        assert_eq!(
            notices,
            ["[chat-bridge WARNING] Your message to coord5 sent 2 min ago (\"message new\") was \
              quarantined as a PROBABLE MISROUTE: it may have reached another pane, or no agent \
              at all, and it will not be sent again by itself. Why: pane wJ:pGY: PROBABLE \
              MISROUTE, quarantined: target shows it. If coord5 does not act on it, send it \
              again."]
        );
        assert!(alarm.due(&entries, "coord5", 13 * 60_000).is_empty());
    }
}
