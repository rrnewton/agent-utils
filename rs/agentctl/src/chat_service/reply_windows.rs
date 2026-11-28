//! The service side of reply windows (#2338 chat-bridge-reply-window-retire): the periodic sweep
//! that closes requests whose window ended, telling about every unanswered request closed, and
//! the owner alarm about a full request population.
//!
//! Each pass of the owner loop, TUI or headless, calls [`ReplyWindowWatch::step`]. Every
//! [`SWEEP_INTERVAL`] it sweeps ([`BridgeState::close_settled_replies`]); every
//! [`CHECK_INTERVAL`] it drains the closure outbox, where the sweep and admission's forced closes
//! list each unanswered request before they close it: each request is logged, one notice listing
//! them is queued to the bridge's agent, and one owner-alarm notice lists the owner's own. Each
//! step is recorded in the outbox once done, so a restart neither repeats nor skips one. It also
//! asks the owner alarm whether the full-population warning or its all-clear is due.

use std::time::{Duration, Instant};

use super::owner_alarm::{self, Notice, OwnerAlarm};
use super::{post_alarm_notices, reborrow, save_owner_alarm, service_log, RootMessageTransport};
use crate::agent::DrainOptions;
use crate::chat_runtime::reply_windows::{
    ClosureBatch, ClosureCause, ClosureStep, ReplyWindowRules, SWEEP_INTERVAL,
};
use crate::chat_runtime::{self, BridgeState};
use crate::subagents::{ManagedAgents, ManagedApi};

/// How often the closure outbox and the capacity hold are looked at.
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// The longest a closure notice waits for the agent to be ready before it is left queued for the
/// next drain to type.
const AGENT_NOTICE_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Closed requests one agent notice lists by name; it counts the rest.
const MAX_LISTED: usize = 50;

/// When the next sweep and the next look at the outbox and hold are due. Kept across passes, so a
/// pass restarted after a transient failure does not sweep again at once.
#[derive(Debug)]
pub(crate) struct ReplyWindowWatch {
    next_sweep: Option<Instant>,
    next_check: Instant,
    high_water: HighWater,
    /// The capacity hold whose alarm the agent was given, by when it began (#2440).
    hold_told: Option<u64>,
}

impl Default for ReplyWindowWatch {
    fn default() -> Self {
        Self {
            next_sweep: None,
            next_check: Instant::now(),
            high_water: HighWater::default(),
            hold_told: None,
        }
    }
}

/// How often at most the agent is told the store crossed its high-water mark.
pub(crate) const HIGH_WATER_REPEAT: Duration = Duration::from_secs(60 * 60);

/// Where the request store stands against its high-water mark, as last told to the agent.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct HighWater {
    /// The agent was told the store is nearly full, and has had no all-clear since.
    above: bool,
    /// When the last nearly-full notice was queued, in Unix milliseconds.
    last_up_millis: Option<u64>,
}

/// Tell the agent through `notify(body, message_id)` once the store fills to the high-water
/// mark, before any refusal, and once it falls back below the low-water mark. A crossing within
/// [`HIGH_WATER_REPEAT`] of the last nearly-full notice is told once that time has passed, if the
/// store is still that full. A notice that cannot be queued is tried again at the next check.
pub(crate) fn high_water_step(
    high_water: &mut HighWater,
    count: u64,
    limit: u64,
    rules: ReplyWindowRules,
    now_millis: u64,
    notify: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    log: &mut dyn FnMut(String),
) {
    if limit == 0 {
        return;
    }
    let percent = count.saturating_mul(100) / limit;
    let repeat = u64::try_from(HIGH_WATER_REPEAT.as_millis()).unwrap_or(u64::MAX);
    let full = count.saturating_mul(100) >= limit.saturating_mul(rules.high_water_percent);
    let clear = count.saturating_mul(100) < limit.saturating_mul(rules.low_water_percent);
    if !high_water.above && full {
        if high_water
            .last_up_millis
            .is_some_and(|at| now_millis.saturating_sub(at) < repeat)
        {
            return;
        }
        let body = format!(
            "[chat-bridge WARNING] The chat bridge's request store is {percent}% full ({count} of \
             {limit} requests kept). At 100% new owner messages are held unacknowledged until \
             space frees. Answered requests retire {} h after their last reply and unanswered \
             ones {} h after admission; to free space now, close requests you have finished \
             with `agentctl chat close --request KEY` (`agentctl chat status` lists them).",
            rules.answered_quiet_millis / 3_600_000,
            rules.unanswered_max_age_millis / 3_600_000,
        );
        match notify(&body, &format!("chat-high-water-up-{now_millis}")) {
            Ok(()) => {
                log(format!(
                    "agentctl: warning: chat request store {percent}% full ({count} of {limit}); \
                     the agent was told"
                ));
                high_water.above = true;
                high_water.last_up_millis = Some(now_millis);
            }
            Err(error) => log(format!(
                "agentctl: warning: chat request store {percent}% full; the notice could not be \
                 queued to the agent, and is tried again: {error}"
            )),
        }
    } else if high_water.above && clear {
        let body = format!(
            "[chat-bridge] The chat bridge's request store is back below {}% ({count} of {limit} \
             requests kept).",
            rules.low_water_percent
        );
        match notify(&body, &format!("chat-high-water-down-{now_millis}")) {
            Ok(()) => {
                log(format!(
                    "agentctl: chat request store back below {}% ({count} of {limit}); the agent \
                     was told",
                    rules.low_water_percent
                ));
                high_water.above = false;
            }
            Err(error) => log(format!(
                "agentctl: warning: the request store's all-clear could not be queued to the \
                 agent, and is tried again: {error}"
            )),
        }
    }
}

/// Put ONE alarm line in the agent's pane, at once, when the request store fills and new owner
/// messages are held (#2440 chat-bridge-never-halt, lead-014: on 2026-11-26 the full store held
/// twelve owner messages for about two hours with only a log line and an hourly owner alarm to
/// show for it). Once per hold, by when it began; the notice's message ID is the hold's, so a
/// restarted service queues it no twice. A notice that cannot be queued is tried again at the
/// next check.
pub(crate) fn capacity_hold_step(
    told: &mut Option<u64>,
    hold: Option<&chat_runtime::RequestCapacityHold>,
    now_millis: u64,
    notify: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    log: &mut dyn FnMut(String),
) {
    let Some(hold) = hold else {
        *told = None;
        return;
    };
    if *told == Some(hold.held_since_millis) {
        return;
    }
    let minutes = now_millis.saturating_sub(hold.held_since_millis) / 60_000;
    let body = format!(
        "[chat-bridge ALARM] The chat bridge's request store is FULL ({} of {} requests kept): {} \
         new owner message(s) are held unacknowledged, for {minutes} min so far. They are admitted \
         as soon as space frees: close requests you have finished with `agentctl chat close \
         --request KEY` (`agentctl chat status` lists them).",
        hold.request_count, hold.max_requests, hold.waiting_messages
    );
    match notify(
        &body,
        &format!("chat-capacity-hold-{}", hold.held_since_millis),
    ) {
        Ok(()) => {
            log(format!(
                "agentctl: error: chat request store full ({} of {}); {} owner message(s) held; \
                 the agent was given the alarm",
                hold.request_count, hold.max_requests, hold.waiting_messages
            ));
            *told = Some(hold.held_since_millis);
        }
        Err(error) => log(format!(
            "agentctl: error: chat request store full; the alarm could not be queued to the \
             agent, and is tried again: {error}"
        )),
    }
}

impl ReplyWindowWatch {
    /// Sweep, tell and alarm when each is due.
    pub(crate) fn step<A: ManagedApi + ?Sized>(
        &mut self,
        state: &BridgeState,
        manager: &ManagedAgents<'_, A>,
        mut alarm: Option<&mut OwnerAlarm>,
        mut transport: Option<&mut dyn RootMessageTransport>,
    ) {
        let now = Instant::now();
        if self.next_sweep.is_none_or(|at| now >= at) {
            self.next_sweep = Some(now + SWEEP_INTERVAL);
            sweep(state, chat_runtime::unix_millis(), &mut |line| {
                service_log(line)
            });
            self.next_check = now;
        }
        if now < self.next_check {
            return;
        }
        self.next_check = now + CHECK_INTERVAL;
        let agent = state.config().agent_name.clone();
        let mut notify = |body: &str, message_id: &str| {
            let options = DrainOptions {
                ready_timeout: AGENT_NOTICE_READY_TIMEOUT,
                ..DrainOptions::default()
            };
            manager
                .send_identified(&agent, body, options, Some(message_id), None)
                .map(|_| ())
                .map_err(|error| error.to_string())
        };
        tell_closures(
            state,
            alarm.as_deref_mut(),
            &mut notify,
            &mut |alarm, notices| {
                post_alarm_notices(alarm, notices, state, reborrow(&mut transport));
            },
            &mut |line| service_log(line),
        );
        match state.request_population() {
            Ok((count, _)) => high_water_step(
                &mut self.high_water,
                count,
                chat_runtime::MAX_REQUESTS_LIMIT,
                state.reply_window_rules(),
                chat_runtime::unix_millis(),
                &mut notify,
                &mut |line| service_log(line),
            ),
            Err(error) => service_log(format_args!(
                "agentctl: warning: chat reply windows: the request population cannot be read: \
                 {error}"
            )),
        }
        capacity_hold_step(
            &mut self.hold_told,
            state.request_capacity_hold().as_ref(),
            chat_runtime::unix_millis(),
            &mut notify,
            &mut |line| service_log(line),
        );
        // A provider gap the bridge kept running through: one pane line, one owner notice
        // (#2440 D5).
        {
            let has_alarm = alarm.is_some();
            let mut post_owner = |notice: Notice| {
                if let Some(alarm) = alarm.as_deref_mut() {
                    post_alarm_notices(alarm, vec![notice], state, reborrow(&mut transport));
                }
            };
            super::gap_reconcile::gap_reconcile_step(
                state,
                chat_runtime::unix_millis(),
                &mut notify,
                has_alarm.then_some(&mut post_owner as &mut dyn FnMut(Notice)),
                &mut |line| service_log(line),
            );
        }
        if let Some(alarm) = alarm {
            capacity_alarm(
                state,
                alarm,
                chat_runtime::unix_millis(),
                &mut |alarm, notices| {
                    post_alarm_notices(alarm, notices, state, reborrow(&mut transport));
                },
            );
        }
    }
}

/// Close the requests whose window ended at `now_millis`, logging what closed.
pub(crate) fn sweep(state: &BridgeState, now_millis: u64, log: &mut dyn FnMut(String)) {
    match state.close_settled_replies(now_millis) {
        Ok(sweep) if sweep.answered + sweep.unanswered > 0 => log(format!(
            "agentctl: chat reply windows: closed {} answered and {} unanswered request(s) whose \
             window ended; {} retired at once",
            sweep.answered, sweep.unanswered, sweep.retired
        )),
        Ok(_) => {}
        Err(error) => log(format!(
            "agentctl: warning: chat reply windows: the sweep failed, and runs again in {} min: \
             {error}",
            SWEEP_INTERVAL.as_secs() / 60
        )),
    }
}

/// The notice for the bridge's agent listing the unanswered requests `batch` closed.
pub(crate) fn agent_notice(batch: &ClosureBatch, unanswered_max_age_millis: u64) -> String {
    let why = match batch.cause {
        ClosureCause::Age => format!(
            "got no reply for {} h, so the chat bridge closed their reply windows",
            unanswered_max_age_millis / 3_600_000
        ),
        ClosureCause::Capacity => "got no reply, and the chat bridge closed their reply windows \
             early to make room for new messages (its request store was full)"
            .to_owned(),
    };
    let mut lines = Vec::new();
    // An owner's request closed to make room while it still awaited the agent's reply: an alarm
    // line each, naming its key, so the agent answers it by publish at once (#2440, lead-014).
    if batch.cause == ClosureCause::Capacity {
        for request in batch
            .requests
            .iter()
            .filter(|request| request.from_owner)
            .take(MAX_LISTED)
        {
            lines.push(format!(
                "[chat-bridge ALARM] The request store was full, so the chat bridge closed OWNER \
                 request {} (reply ID {}) while it still awaited your reply: answer it now in its \
                 thread with `agentctl chat publish`.",
                request.key, request.reply_id
            ));
        }
    }
    lines.push(format!(
        "[chat-bridge] {} chat request(s) {why}; their reply IDs take no more replies, and the \
         owner was told about their own:",
        batch.requests.len()
    ));
    for request in batch.requests.iter().take(MAX_LISTED) {
        lines.push(format!(
            "- reply ID {} (sent {}{}): \"{}\"",
            request.reply_id,
            crate::chat_trace::et_clock((request.admitted_at_millis / 1_000) as f64),
            if request.from_owner {
                ", from the owner"
            } else {
                ""
            },
            request.first_line
        ));
    }
    let more = batch.requests.len().saturating_sub(MAX_LISTED);
    if more > 0 {
        lines.push(format!("- and {more} more"));
    }
    lines.push(
        "If one still needs an answer, answer it in its thread with `agentctl chat publish`."
            .to_owned(),
    );
    lines.join("\n")
}

/// Tell about every closure batch in the outbox: log each request, queue one notice to the agent
/// through `notify(body, message_id)`, and hand one owner-alarm notice to `post`. A step that
/// fails is tried again at the next check; one that succeeded is recorded and never repeated.
pub(crate) fn tell_closures(
    state: &BridgeState,
    mut alarm: Option<&mut OwnerAlarm>,
    notify: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    post: &mut dyn FnMut(&mut OwnerAlarm, Vec<Notice>),
    log: &mut dyn FnMut(String),
) {
    let (batches, dropped) = match state.closure_batches() {
        Ok(found) => found,
        Err(error) => {
            log(format!(
                "agentctl: warning: chat reply windows: the closure list cannot be read: {error}"
            ));
            return;
        }
    };
    let rules = state.reply_window_rules();
    let mut clear_dropped = dropped > 0;
    if clear_dropped {
        log(format!(
            "agentctl: warning: chat reply windows: {dropped} closure batch(es) were dropped \
             untold because the closure list was full"
        ));
    }
    let agent_label = state.config().agent_label.clone();
    for batch in batches {
        let mut mark = |step: ClosureStep, log: &mut dyn FnMut(String)| {
            if let Err(error) = state.mark_closure_batch(&batch.id, step, clear_dropped) {
                log(format!(
                    "agentctl: warning: chat reply windows: closure batch {} could not be \
                     updated: {error}",
                    batch.id
                ));
            }
            clear_dropped = false;
        };
        if !batch.logged {
            let cause = match batch.cause {
                ClosureCause::Age => "their window ended",
                ClosureCause::Capacity => "the request store was full",
            };
            if batch.answered_closed > 0 {
                log(format!(
                    "agentctl: chat reply windows: closed {} answered request(s) because {cause}",
                    batch.answered_closed
                ));
            }
            for request in &batch.requests {
                log(format!(
                    "agentctl: chat reply windows: closed UNANSWERED request {} (reply ID {}, \
                     from {}{}, admitted {} Unix ms) because {cause}: \"{}\"",
                    request.key,
                    request.reply_id,
                    request.sender_id,
                    if request.from_owner { ", an owner" } else { "" },
                    request.admitted_at_millis,
                    request.first_line
                ));
            }
            mark(ClosureStep::Logged, log);
        }
        if !batch.agent_notified {
            let body = agent_notice(&batch, rules.unanswered_max_age_millis);
            let message_id = format!("chat-window-closed-{}", batch.id);
            match notify(&body, &message_id) {
                Ok(()) => mark(ClosureStep::AgentNotified, log),
                Err(error) => log(format!(
                    "agentctl: warning: chat reply windows: the notice about {} closed \
                     request(s) could not be queued to the agent, and is tried again: {error}",
                    batch.requests.len()
                )),
            }
        }
        if !batch.owner_notified {
            let notice =
                owner_alarm::closure_notice(&batch, &agent_label, rules.unanswered_max_age_millis);
            match (notice, alarm.as_deref_mut()) {
                (Some(notice), Some(alarm)) => post(alarm, vec![notice]),
                (Some(notice), None) => log(format!(
                    "agentctl: chat reply windows: owner alarm off; not posted: {}",
                    notice.text
                )),
                (None, _) => {}
            }
            mark(ClosureStep::OwnerNotified, log);
        }
    }
}

/// Hand the owner alarm's full-population warning or all-clear to `post` when one is due, and
/// save the alarm when its state changed.
pub(crate) fn capacity_alarm(
    state: &BridgeState,
    alarm: &mut OwnerAlarm,
    now_millis: u64,
    post: &mut dyn FnMut(&mut OwnerAlarm, Vec<Notice>),
) {
    super::restore_owner_alarm(alarm, state);
    let before = alarm.capacity().cloned();
    let hold = state.request_capacity_hold();
    let notices = alarm.capacity_due(hold.as_ref(), &state.config().agent_label, now_millis);
    if let Some(hold) = &hold {
        if !notices.is_empty() {
            service_log(format_args!(
                "agentctl: error: chat request store full: {} of {} requests kept, {} new \
                 message(s) held uncommitted since {} Unix ms ({} refusal(s)); warning the owner",
                hold.request_count,
                hold.max_requests,
                hold.waiting_messages,
                hold.held_since_millis,
                hold.refusals
            ));
        }
    }
    if !notices.is_empty() {
        post(alarm, notices);
    }
    if alarm.capacity() != before.as_ref() {
        save_owner_alarm(alarm, state);
    }
}
