//! The team-restore driver's state machine as data, and its exhaustive
//! checker: the same transition table the driver interprets is the table
//! this module proves things about.
//!
//! The owner asked for every transition to be model-checked because the
//! failure that matters is a stall: locking and paperwork added around a
//! process that can still park forever. So the checker's centre is
//! liveness under weak fairness. The fair events are the ones the machine
//! itself guarantees: an armed wake timer eventually fires, and some
//! escalation route eventually works. Everything benevolent a human or a
//! lead might do (fix the underlying problem, kick by hand) is unfair: it
//! may never happen, and the machine must still reach a verifying team or
//! a delivered escalation without it. Concretely, liveness fails exactly
//! when a reachable state cannot reach a target (healthy, or escalation
//! delivered) using only driver steps plus fair events; the path to such
//! a state is the counterexample. The two seeded tables in the tests
//! (HANDOFF without its deadline row, and exit without re-arming the
//! wake) must each strand HANDOFF that way.
//!
//! Safety is checked on the full graph, unfair environment included: a
//! dormant driver is only ever healthy or wake-armed (resident waiting
//! abstracts as armed, as the freeze records), restore is never launched
//! twice for one incident without the first finishing, and the only
//! launch row is DETECT finding desired agents missing (the desired set
//! itself, parked agents excluded, is `team_verify`'s derivation).
//! Counterexamples serialize as `agentctl-team-trace/v1` so the driver's
//! tests can replay them against the real step function.
//!
//! Bounds are small and exhaustive, never timed: environment facts are
//! four booleans, attempts saturate, and the visited cap is a state
//! count, so the checker is as robust on a hot machine as a cold one.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

/// Driver states of the minimal subset (full-model states join later).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Verified healthy; a kick reports healthy and changes nothing.
    Healthy,
    /// Compare desired, live, and unit state.
    Detect,
    /// Revive every desired agent (today's restore, replay never trusted).
    Restore,
    /// Run the team-verify checks.
    Verify,
    /// The lead owns the remaining checklist; the driver watches.
    Handoff,
    /// Deliver to a human by configured command; keep re-verifying.
    Escalate,
}

/// Inputs the machine reacts to. Detection and verification outcomes are
/// distinct events, so the table needs no guard language: whichever
/// outcome the world produced is the event that fires.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    /// Any kicker, any number of times, in any state.
    Kick,
    /// The armed re-entry timer fired (resident waiting included).
    WakeFired,
    /// Boot plus user-manager identity changed: a new incident.
    NewEpoch,
    /// DETECT found nothing missing.
    DetectClean,
    /// DETECT found desired agents or units missing.
    DetectMissing,
    /// The restore call finished, whatever its per-lane class.
    RestoreDone,
    /// Every verify check passed (in VERIFY, or re-probed in ESCALATE).
    VerifyPass,
    /// Verify failed and a lead can be handed the checklist.
    VerifyFailLead,
    /// Verify failed and no lead can take it.
    VerifyFailNoLead,
    /// Verify's only failure is a coordinator notice awaiting delivery
    /// before its acknowledgement deadline.
    VerifyAckPending,
    /// A HANDOFF re-probe finds the team fixed.
    Progress,
    /// A persisted deadline (handoff, backoff, reminder) is reached.
    DeadlineReached,
    /// The lead died or became unreachable during HANDOFF.
    LeadGone,
    /// Some escalation route exited 0.
    EscalateDelivered,
    /// Every escalation route failed.
    EscalateFailed,
    /// The driver process died; re-entry repeats the persisted step.
    Crash,
}

/// Effects a transition asks the driver to perform, in order. State is
/// persisted before all of them; each is idempotent.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    /// Call restore with the replay guard bypassed for this detection.
    LaunchRestore,
    /// Run the verify checks.
    RunVerify,
    /// Queue the remaining checklist to the team lead.
    QueueLeadChecklist,
    /// Try the configured escalation routes in order.
    DeliverEscalation,
    /// Arm the re-entry wake at the state's deadline (or wait resident).
    ArmWake,
    /// Write the undelivered-escalation alarm file.
    WriteAlarm,
    /// Remove the alarm file.
    ClearAlarm,
    /// Send the one resolved escalation, when one was delivered.
    SendResolved,
    /// Exit healthy (0) or recovery-pending (75), per the exit rule.
    Exit,
}

/// Whether a transition leaves the driver running or dormant. Dormant is
/// only ever legal when healthy or wake-armed; the checker proves it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// The driver process is acting (and holds the team flock).
    Running,
    /// The driver has exited; kick, wake, or boot re-enters it.
    Dormant,
}

/// One row of the shared transition table.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    /// State the row fires from.
    pub from: State,
    /// Event that fires it.
    pub event: Event,
    /// State entered.
    pub to: State,
    /// Mode after the transition.
    pub mode: Mode,
    /// Effects to perform, in order.
    pub effects: &'static [Effect],
}

const NONE: &[Effect] = &[];

/// The minimal-subset transition table. The driver interprets exactly
/// these rows; the checker proves exactly these rows. Kick rows exist for
/// every state (a kick probes DETECT and resets no incident fact), and
/// Crash rows for every running state (re-entry repeats the step).
pub fn transitions() -> Vec<Transition> {
    use Effect::*;
    use Event::*;
    use Mode::*;
    use State::*;
    let mut rows = vec![
        Transition {
            from: Healthy,
            event: WakeFired,
            to: Detect,
            mode: Running,
            effects: NONE,
        },
        Transition {
            from: Detect,
            event: DetectClean,
            to: Healthy,
            mode: Dormant,
            effects: &[ClearAlarm, SendResolved, Exit],
        },
        Transition {
            from: Detect,
            event: DetectMissing,
            to: Restore,
            mode: Running,
            effects: &[LaunchRestore],
        },
        Transition {
            from: Restore,
            event: RestoreDone,
            to: Verify,
            mode: Running,
            effects: &[RunVerify],
        },
        Transition {
            from: Verify,
            event: VerifyAckPending,
            to: Verify,
            mode: Dormant,
            effects: &[ArmWake, Exit],
        },
        Transition {
            from: Verify,
            event: WakeFired,
            to: Verify,
            mode: Running,
            effects: &[RunVerify],
        },
        Transition {
            from: Verify,
            event: VerifyPass,
            to: Healthy,
            mode: Dormant,
            effects: &[ClearAlarm, SendResolved, Exit],
        },
        Transition {
            from: Verify,
            event: VerifyFailLead,
            to: Handoff,
            mode: Dormant,
            effects: &[QueueLeadChecklist, ArmWake, Exit],
        },
        Transition {
            from: Verify,
            event: VerifyFailNoLead,
            to: Escalate,
            mode: Running,
            effects: &[DeliverEscalation],
        },
        Transition {
            from: Handoff,
            event: WakeFired,
            to: Handoff,
            mode: Running,
            effects: &[RunVerify],
        },
        Transition {
            from: Handoff,
            event: Progress,
            to: Verify,
            mode: Running,
            effects: &[RunVerify],
        },
        Transition {
            from: Handoff,
            event: DeadlineReached,
            to: Escalate,
            mode: Running,
            effects: &[DeliverEscalation],
        },
        Transition {
            from: Handoff,
            event: LeadGone,
            to: Escalate,
            mode: Running,
            effects: &[DeliverEscalation],
        },
        Transition {
            from: Escalate,
            event: VerifyPass,
            to: Healthy,
            mode: Dormant,
            effects: &[ClearAlarm, SendResolved, Exit],
        },
        Transition {
            from: Escalate,
            event: EscalateDelivered,
            to: Escalate,
            mode: Dormant,
            effects: &[ArmWake, Exit],
        },
        Transition {
            from: Escalate,
            event: EscalateFailed,
            to: Escalate,
            mode: Dormant,
            effects: &[WriteAlarm, ArmWake, Exit],
        },
        Transition {
            from: Escalate,
            event: WakeFired,
            to: Escalate,
            mode: Running,
            effects: &[RunVerify],
        },
        Transition {
            from: Escalate,
            event: DeadlineReached,
            to: Escalate,
            mode: Running,
            effects: &[DeliverEscalation],
        },
    ];
    for state in [Healthy, Detect, Restore, Verify, Handoff, Escalate] {
        rows.push(Transition {
            from: state,
            event: Kick,
            to: Detect,
            mode: Running,
            effects: NONE,
        });
        rows.push(Transition {
            from: state,
            event: NewEpoch,
            to: Detect,
            mode: Running,
            effects: NONE,
        });
        rows.push(Transition {
            from: state,
            event: Crash,
            to: state,
            mode: Running,
            effects: NONE,
        });
    }
    rows
}

/// Facts outside the machine's control, as booleans at checker bounds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Env {
    /// Desired agents or units are missing (DETECT's input).
    pub missing: bool,
    /// The team lead is reachable.
    pub lead_up: bool,
    /// Some escalation route currently works.
    pub route_ok: bool,
    /// The underlying problem is fixed, so verify would pass.
    pub fixed: bool,
}

/// One abstract machine state: everything the checker must distinguish.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Abstract {
    /// Driver state.
    pub driver: State,
    /// Running or dormant.
    pub mode: Mode,
    /// A wake is armed (timer, or resident waiting abstracted as armed).
    pub wake_armed: bool,
    /// The persisted deadline has been reached on this re-entry.
    pub deadline_expired: bool,
    /// A restore launch is still in flight.
    pub restore_in_flight: bool,
    /// An escalation was delivered this incident.
    pub delivered: bool,
    /// The undelivered alarm file is written.
    pub alarm: bool,
    /// Escalation attempts this incident, saturating at 3.
    pub attempts: u8,
    /// Environment facts.
    pub env: Env,
}

impl Abstract {
    /// Dormant and healthy under one environment: where kicks land.
    pub fn initial(env: Env) -> Self {
        Self {
            driver: State::Healthy,
            mode: Mode::Dormant,
            wake_armed: false,
            deadline_expired: false,
            restore_in_flight: false,
            delivered: false,
            alarm: false,
            attempts: 0,
            env,
        }
    }

    /// Liveness target: verified healthy, or a human holds the escalation.
    pub fn is_target(&self) -> bool {
        self.delivered || self.driver == State::Healthy
    }
}

/// Whether `event` can occur in `state` at all, given the environment.
/// This is the world's half of the contract; the table is the machine's.
fn event_enabled(state: &Abstract, event: Event) -> bool {
    use Event::*;
    use State::*;
    match event {
        Kick | NewEpoch => true,
        WakeFired => state.mode == Mode::Dormant && state.wake_armed,
        Crash => state.mode == Mode::Running,
        DetectClean => state.driver == Detect && !state.env.missing,
        DetectMissing => state.driver == Detect && state.env.missing,
        RestoreDone => state.driver == Restore && state.restore_in_flight,
        VerifyPass => matches!(state.driver, Verify | Escalate) && state.env.fixed,
        VerifyFailLead => state.driver == Verify && !state.env.fixed && state.env.lead_up,
        VerifyFailNoLead => state.driver == Verify && !state.env.fixed && !state.env.lead_up,
        VerifyAckPending => state.driver == Verify && !state.deadline_expired,
        Progress => state.driver == Handoff && state.env.fixed,
        DeadlineReached => matches!(state.driver, Handoff | Escalate) && state.deadline_expired,
        LeadGone => state.driver == Handoff && !state.env.lead_up,
        EscalateDelivered => state.driver == Escalate && state.env.route_ok,
        EscalateFailed => state.driver == Escalate && !state.env.route_ok,
    }
}

/// Fair events: guaranteed by the machine or assumed of the world. An
/// armed wake fires; some escalation route eventually works (modelled as
/// the environment step that sets `route_ok`). Kicks, epochs, crashes,
/// lead deaths, and underlying fixes are unfair: they may never happen.
fn is_fair(event: Event) -> bool {
    !matches!(event, Event::Kick | Event::NewEpoch | Event::Crash)
}

/// One table step: the row for `(driver, event)`, with incident facts
/// updated exactly as the driver updates its persisted state. `None`
/// when the event cannot occur or the table has no row for it (itself a
/// stall the checker reports through liveness).
pub fn step(
    table: &[Transition],
    state: &Abstract,
    event: Event,
) -> Option<(Abstract, Vec<Effect>)> {
    if !event_enabled(state, event) {
        return None;
    }
    if state.mode == Mode::Dormant
        && !matches!(event, Event::Kick | Event::WakeFired | Event::NewEpoch)
    {
        return None;
    }
    // A kick to a running driver only reports its state (the flock is
    // held); it never re-probes or relaunches anything.
    if event == Event::Kick && state.mode == Mode::Running {
        return Some((*state, Vec::new()));
    }
    let row = table
        .iter()
        .find(|row| row.from == state.driver && row.event == event)?;
    let mut next = *state;
    next.driver = row.to;
    next.mode = row.mode;
    match event {
        Event::NewEpoch => {
            next.delivered = false;
            next.alarm = false;
            next.attempts = 0;
            next.deadline_expired = false;
            next.wake_armed = false;
            next.restore_in_flight = false;
        }
        Event::WakeFired => {
            next.wake_armed = false;
            next.deadline_expired = matches!(
                state.driver,
                State::Verify | State::Handoff | State::Escalate
            );
        }
        Event::RestoreDone => next.restore_in_flight = false,
        Event::EscalateDelivered => {
            next.delivered = true;
            next.attempts = next.attempts.saturating_add(1).min(3);
            next.deadline_expired = false;
        }
        Event::EscalateFailed => {
            next.attempts = next.attempts.saturating_add(1).min(3);
            next.deadline_expired = false;
        }
        Event::DeadlineReached => next.deadline_expired = false,
        _ => {}
    }
    for effect in row.effects {
        match effect {
            Effect::LaunchRestore => next.restore_in_flight = true,
            Effect::ArmWake => next.wake_armed = true,
            Effect::WriteAlarm => next.alarm = true,
            Effect::ClearAlarm => next.alarm = false,
            _ => {}
        }
    }
    if row.to == State::Healthy {
        next.wake_armed = false;
        next.deadline_expired = false;
        next.restore_in_flight = false;
        next.alarm = false;
    }
    if row.to == State::Handoff && state.driver == State::Verify {
        next.deadline_expired = false;
    }
    Some((next, row.effects.to_vec()))
}

/// Environment steps between machine steps: the world changing its mind.
/// Only the route recovering is fair; a lead dying or the problem being
/// fixed underneath may never happen, and liveness must not need them.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvStep {
    /// The underlying problem gets fixed (verify would now pass).
    EnvFix,
    /// The lead dies or becomes unreachable.
    LeadDies,
    /// An escalation route starts working. The one fair env step.
    RouteFixes,
}

fn env_step(state: &Abstract, step_kind: EnvStep) -> Option<Abstract> {
    let mut next = *state;
    match step_kind {
        EnvStep::EnvFix if !state.env.fixed => {
            next.env.fixed = true;
            next.env.missing = false;
        }
        EnvStep::LeadDies if state.env.lead_up => next.env.lead_up = false,
        EnvStep::RouteFixes if !state.env.route_ok => next.env.route_ok = true,
        _ => return None,
    }
    Some(next)
}

/// Every event worth trying from a state, table or environment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Input {
    /// A table event.
    Event {
        /// The event.
        event: Event,
    },
    /// An environment step.
    Env {
        /// The step.
        step: EnvStep,
    },
}

impl Input {
    fn is_fair(self) -> bool {
        match self {
            Input::Event { event } => is_fair(event),
            Input::Env { step } => step == EnvStep::RouteFixes,
        }
    }
}

fn apply(table: &[Transition], state: &Abstract, input: Input) -> Option<(Abstract, Vec<Effect>)> {
    match input {
        Input::Event { event } => step(table, state, event),
        Input::Env { step: kind } => env_step(state, kind).map(|next| (next, Vec::new())),
    }
}

const ALL_EVENTS: [Event; 16] = [
    Event::Kick,
    Event::WakeFired,
    Event::NewEpoch,
    Event::DetectClean,
    Event::DetectMissing,
    Event::RestoreDone,
    Event::VerifyPass,
    Event::VerifyFailLead,
    Event::VerifyFailNoLead,
    Event::VerifyAckPending,
    Event::Progress,
    Event::DeadlineReached,
    Event::LeadGone,
    Event::EscalateDelivered,
    Event::EscalateFailed,
    Event::Crash,
];

const ALL_ENV_STEPS: [EnvStep; 3] = [EnvStep::EnvFix, EnvStep::LeadDies, EnvStep::RouteFixes];

/// One recorded step of a counterexample or replay trace.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TraceStep {
    /// Abstract state before the input.
    pub from: Abstract,
    /// The input applied.
    pub input: Input,
    /// Abstract state after it.
    pub to: Abstract,
    /// Effects the step emitted.
    pub effects: Vec<Effect>,
}

/// A `agentctl-team-trace/v1` document: replayable against the real
/// driver by mapping each event to the fake-host outcome of its name.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Trace {
    /// Always `agentctl-team-trace/v1`.
    pub schema: String,
    /// Which seeded mutation produced it, when it is a seed counterexample.
    pub seed: Option<String>,
    /// The steps, from an initial state.
    pub steps: Vec<TraceStep>,
}

impl Trace {
    /// Re-run every step against `table`, refusing any step the machine
    /// would not take identically. Returns the final abstract state.
    pub fn replay(&self, table: &[Transition]) -> Result<Abstract, String> {
        let mut current = self
            .steps
            .first()
            .map(|step| step.from)
            .ok_or("trace has no steps")?;
        for (index, recorded) in self.steps.iter().enumerate() {
            if recorded.from != current {
                return Err(format!("step {index} does not continue the trace"));
            }
            let (next, effects) = apply(table, &current, recorded.input)
                .ok_or_else(|| format!("step {index} is not enabled"))?;
            if next != recorded.to || effects != recorded.effects {
                return Err(format!("step {index} replays differently"));
            }
            current = next;
        }
        Ok(current)
    }
}

/// What checking a table proved, in the gate's vocabulary.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Abstract states explored.
    pub states: usize,
    /// Table rows the exploration actually fired.
    pub rows_fired: usize,
}

/// Why a table failed: a reachable state no fair path leads out of, with
/// the shortest recorded path into it.
#[derive(Clone, Debug, PartialEq)]
pub struct Counterexample {
    /// The stranded state.
    pub stranded: Abstract,
    /// How the machine got there.
    pub trace: Trace,
}

/// Every initial environment: dormant and healthy under all 16 worlds.
fn initials() -> Vec<Abstract> {
    let mut states = Vec::new();
    for missing in [false, true] {
        for lead_up in [false, true] {
            for route_ok in [false, true] {
                for fixed in [false, true] {
                    states.push(Abstract::initial(Env {
                        missing,
                        lead_up,
                        route_ok,
                        fixed,
                    }));
                }
            }
        }
    }
    states
}

/// Check `table`: full-graph safety, fair-graph liveness. The visited
/// cap is a state count (bounds above keep the space near 50k), never a
/// clock, so a hot machine changes nothing.
pub fn check(table: &[Transition]) -> Result<Report, Counterexample> {
    const VISITED_CAP: usize = 200_000;
    let mut visited: HashSet<Abstract> = HashSet::new();
    let mut parents: HashMap<Abstract, (Abstract, Input, Vec<Effect>)> = HashMap::new();
    let mut queue: VecDeque<Abstract> = VecDeque::new();
    let mut fired: HashSet<(State, Event)> = HashSet::new();
    let mut fair_edges: Vec<(Abstract, Abstract)> = Vec::new();
    for start in initials() {
        if visited.insert(start) {
            queue.push_back(start);
        }
    }
    let inputs = |state: &Abstract| -> Vec<(Input, Abstract, Vec<Effect>)> {
        let mut out = Vec::new();
        for event in ALL_EVENTS {
            let input = Input::Event { event };
            if let Some(found) = apply(table, state, input) {
                out.push((input, found.0, found.1));
            }
        }
        for kind in ALL_ENV_STEPS {
            let input = Input::Env { step: kind };
            if let Some(found) = apply(table, state, input) {
                out.push((input, found.0, found.1));
            }
        }
        out
    };
    while let Some(state) = queue.pop_front() {
        // Safety, on every reachable state: dormant only when healthy or
        // armed; an armed wake is the only way a deadline exists at all.
        if state.mode == Mode::Dormant && state.driver != State::Healthy && !state.wake_armed {
            return Err(counterexample(&parents, state, None));
        }
        for (input, next, effects) in inputs(&state) {
            if let Input::Event { event } = input {
                fired.insert((state.driver, event));
                // Safety: never launch a second restore over an in-flight one.
                if effects.contains(&Effect::LaunchRestore) && state.restore_in_flight {
                    return Err(counterexample(&parents, state, None));
                }
            }
            if input.is_fair() {
                fair_edges.push((state, next));
            }
            if visited.insert(next) {
                assert!(visited.len() <= VISITED_CAP, "state space past its bound");
                parents.insert(next, (state, input, effects));
                queue.push_back(next);
            }
        }
    }
    // Liveness: reverse-reach targets over fair edges only. A visited
    // non-target outside that set can stall forever under weak fairness.
    let mut reaching: HashSet<Abstract> = visited
        .iter()
        .copied()
        .filter(Abstract::is_target)
        .collect();
    let mut reverse: HashMap<Abstract, Vec<Abstract>> = HashMap::new();
    for (from, to) in &fair_edges {
        reverse.entry(*to).or_default().push(*from);
    }
    let mut frontier: VecDeque<Abstract> = reaching.iter().copied().collect();
    while let Some(state) = frontier.pop_front() {
        for before in reverse.get(&state).into_iter().flatten() {
            if reaching.insert(*before) {
                frontier.push_back(*before);
            }
        }
    }
    let stranded = visited
        .iter()
        .filter(|state| !reaching.contains(state))
        .min_by_key(|state| (state.driver != State::Handoff, trace_len(&parents, state)));
    match stranded {
        None => Ok(Report {
            states: visited.len(),
            rows_fired: fired.len(),
        }),
        Some(state) => Err(counterexample(&parents, *state, None)),
    }
}

fn trace_len(
    parents: &HashMap<Abstract, (Abstract, Input, Vec<Effect>)>,
    state: &Abstract,
) -> usize {
    let mut length = 0;
    let mut current = state;
    while let Some((before, _, _)) = parents.get(current) {
        length += 1;
        current = before;
    }
    length
}

fn counterexample(
    parents: &HashMap<Abstract, (Abstract, Input, Vec<Effect>)>,
    stranded: Abstract,
    seed: Option<String>,
) -> Counterexample {
    let mut steps: Vec<TraceStep> = Vec::new();
    let mut current = stranded;
    while let Some((before, input, effects)) = parents.get(&current) {
        steps.push(TraceStep {
            from: *before,
            input: *input,
            to: current,
            effects: effects.clone(),
        });
        current = *before;
    }
    steps.reverse();
    Counterexample {
        stranded,
        trace: Trace {
            schema: "agentctl-team-trace/v1".to_owned(),
            seed,
            steps,
        },
    }
}

/// A seeded mutation of the real table. Each must fail the checker with
/// a HANDOFF stall; a checker that passes a seed proves nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Seed {
    /// HANDOFF loses its deadline exit: only benevolence can move it.
    NoHandoffDeadline,
    /// HANDOFF entry exits without arming a wake: nobody returns.
    NoRearm,
}

impl Seed {
    /// The seed's stable name, recorded in counterexample traces.
    pub fn name(self) -> &'static str {
        match self {
            Seed::NoHandoffDeadline => "no-handoff-deadline",
            Seed::NoRearm => "no-rearm",
        }
    }
}

/// The real table with one deliberate break, for checker self-tests.
pub fn seeded_table(seed: Seed) -> Vec<Transition> {
    let mut table = transitions();
    match seed {
        Seed::NoHandoffDeadline => {
            table.retain(|row| !(row.from == State::Handoff && row.event == Event::DeadlineReached))
        }
        Seed::NoRearm => {
            for row in &mut table {
                if row.from == State::Verify && row.event == Event::VerifyFailLead {
                    row.effects = &[Effect::QueueLeadChecklist, Effect::Exit];
                }
            }
        }
    }
    table
}

/// Check a seeded table and name the seed in its counterexample.
pub fn check_seeded(seed: Seed) -> Result<Report, Counterexample> {
    check(&seeded_table(seed)).map_err(|mut found| {
        found.trace.seed = Some(seed.name().to_owned());
        found
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(missing: bool, lead_up: bool, route_ok: bool, fixed: bool) -> Env {
        Env {
            missing,
            lead_up,
            route_ok,
            fixed,
        }
    }

    #[test]
    fn real_table_passes_and_fires_every_row_kind() {
        let table = transitions();
        let report = check(&table).expect("real table must pass");
        assert!(report.states > 100, "{report:?}");
        // The eighteen core rows all fire somewhere in the exploration: a
        // dead row would be a transition the design claims but the
        // machine can never take. (Kick, epoch, and crash rows repeat per
        // state and are counted once per (state, event) that occurs.)
        assert!(report.rows_fired >= 18, "{report:?}");
        // The launch effect lives on exactly the DETECT-missing row, so
        // nothing parked or already-live can ever be (re)launched.
        let launches: Vec<&Transition> = table
            .iter()
            .filter(|row| row.effects.contains(&Effect::LaunchRestore))
            .collect();
        assert_eq!(launches.len(), 1);
        assert_eq!(launches[0].from, State::Detect);
        assert_eq!(launches[0].event, Event::DetectMissing);
        // Every dormant exit except healthy arms its wake first.
        for row in &table {
            if row.mode == Mode::Dormant && row.to != State::Healthy {
                assert!(
                    row.effects.contains(&Effect::ArmWake),
                    "dormant exit without ArmWake: {row:?}"
                );
            }
        }
    }

    #[test]
    fn verify_wake_rechecks_then_can_hand_off() {
        // A sole not-yet-expired coordinator-ack failure waits in VERIFY:
        // the wake re-runs the checks with the ack deadline reached, after
        // which the existing fail rows classify it.
        let table = transitions();
        let running = Abstract {
            driver: State::Verify,
            mode: Mode::Running,
            ..Abstract::initial(env(false, true, true, false))
        };
        let (waiting, pending_effects) =
            step(&table, &running, Event::VerifyAckPending).expect("VERIFY pending row");
        assert_eq!(waiting.mode, Mode::Dormant);
        assert!(waiting.wake_armed);
        assert_eq!(pending_effects, [Effect::ArmWake, Effect::Exit]);
        let (next, effects) = step(&table, &waiting, Event::WakeFired).expect("VERIFY wake row");
        assert_eq!(next.driver, State::Verify);
        assert_eq!(next.mode, Mode::Running);
        assert!(!next.wake_armed);
        assert!(next.deadline_expired);
        assert_eq!(effects, [Effect::RunVerify]);
        let (handoff, _) =
            step(&table, &next, Event::VerifyFailLead).expect("expired ack hands off");
        assert_eq!(handoff.driver, State::Handoff);
        assert_eq!(handoff.mode, Mode::Dormant);
        assert!(handoff.wake_armed);
    }

    #[test]
    fn seed_without_handoff_deadline_strands_handoff() {
        let found = check_seeded(Seed::NoHandoffDeadline).expect_err("seed must fail");
        assert_eq!(found.stranded.driver, State::Handoff, "{found:?}");
        assert_eq!(found.trace.seed.as_deref(), Some("no-handoff-deadline"));
        let table = seeded_table(Seed::NoHandoffDeadline);
        let document = serde_json::to_string(&found.trace).unwrap();
        let parsed: Trace = serde_json::from_str(&document).unwrap();
        assert_eq!(parsed.schema, "agentctl-team-trace/v1");
        assert_eq!(parsed.replay(&table).unwrap(), found.stranded);
        // The same trace is meaningless against the real table only in
        // that the real machine would have left HANDOFF: replaying its
        // recorded steps still works, so assert on the seed name instead.
        assert!(document.contains("no-handoff-deadline"));
    }

    #[test]
    fn seed_without_rearm_strands_handoff() {
        let found = check_seeded(Seed::NoRearm).expect_err("seed must fail");
        assert_eq!(found.stranded.driver, State::Handoff, "{found:?}");
        assert_eq!(found.stranded.mode, Mode::Dormant);
        assert!(!found.stranded.wake_armed);
        let table = seeded_table(Seed::NoRearm);
        assert_eq!(found.trace.replay(&table).unwrap(), found.stranded);
    }

    #[test]
    fn kick_never_resets_incident_facts_or_launches() {
        let table = transitions();
        // Incident facts survive a kick from every initial world, and a
        // kick to a running driver is a pure state report.
        for start in initials() {
            let kicked = step(&table, &start, Event::Kick).expect("kick enabled");
            assert_eq!(kicked.0.delivered, start.delivered);
            assert_eq!(kicked.0.attempts, start.attempts);
            assert!(!kicked.1.contains(&Effect::LaunchRestore));
            assert!(!kicked.1.contains(&Effect::DeliverEscalation));
        }
        let running = Abstract {
            driver: State::Restore,
            mode: Mode::Running,
            restore_in_flight: true,
            delivered: true,
            attempts: 2,
            ..Abstract::initial(env(true, true, true, false))
        };
        let (same, effects) = step(&table, &running, Event::Kick).unwrap();
        assert_eq!(same, running);
        assert!(effects.is_empty());
    }

    #[test]
    fn manager_restart_shape_new_epoch_restarts_detection() {
        // Same boot, new manager: the machine level only ever sees the
        // epoch change, and it must restart detection with clean facts.
        let table = transitions();
        let healthy = Abstract {
            delivered: true,
            attempts: 3,
            alarm: true,
            ..Abstract::initial(env(false, true, true, true))
        };
        let (next, _) = step(&table, &healthy, Event::NewEpoch).unwrap();
        assert_eq!(next.driver, State::Detect);
        assert_eq!(next.mode, Mode::Running);
        assert!(!next.delivered && !next.alarm && next.attempts == 0);
    }
}
