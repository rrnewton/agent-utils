//! Fixture tests for #1418 topology fragments and watcher decisions.
//! Pure fixtures only: no live registry, unit, route or installed binary.

use agent_teams::topology::{
    due, merge, parse_fragment, should_kick, tick, validate_routes, DriverSnapshot, Due, Event,
    IncidentLedger, PeerHealth, PeerIo, Watch,
};

#[derive(Default)]
struct FakeIo {
    snapshot: Option<DriverSnapshot>,
    checkins: usize,
    probes: usize,
    kicks: usize,
    notifies: usize,
}

impl PeerIo for FakeIo {
    fn send_checkin(&mut self, _peer: &str) -> Result<(), String> {
        self.checkins += 1;
        Ok(())
    }
    fn probe_driver(&mut self, _peer: &str) -> Result<DriverSnapshot, String> {
        self.probes += 1;
        self.snapshot
            .clone()
            .ok_or_else(|| "unreachable".to_string())
    }
    fn kick(&mut self, _peer: &str) -> Result<(), String> {
        self.kicks += 1;
        Ok(())
    }
    fn notify(&mut self, _peer: &str, _incident: &str) -> Result<(), String> {
        self.notifies += 1;
        Ok(())
    }
}
use std::collections::BTreeSet;

const TEAM_A: &str = r#"{
  "schema": "agent-utils-topology/v1",
  "team": "team-a",
  "checkin_after_silence_seconds": 21600,
  "probe_after_silence_seconds": 21600,
  "probe_after_unanswered_seconds": 3600,
  "instances": {
    "team-a@host-1": {
      "registry": "~/work/team-a/.agentctl",
      "lead": null,
      "subordinates": ["team-b@host-2"],
      "coordinator_kind": "host"
    }
  }
}"#;

const TEAM_B: &str = r#"{
  "schema": "agent-utils-topology/v1",
  "team": "team-b",
  "instances": {
    "team-b@host-2": {
      "registry": "~/work/team-b/.agentctl",
      "lead": "team-a@host-1",
      "subordinates": [],
      "coordinator_kind": "agentcloud"
    }
  }
}"#;

fn merged() -> agent_teams::topology::Topology {
    let a = parse_fragment(TEAM_A, "team-a").expect("team-a fragment parses");
    let b = parse_fragment(TEAM_B, "team-b").expect("team-b fragment parses");
    merge(vec![a, b], None).expect("fragments merge without conflicts")
}

#[test]
fn silence_triggers_checkin_and_probe() {
    let topology = merged();
    let defaults = topology.defaults_for("team-a");
    let now = 1_000_000;
    let silent = Watch {
        last_seen_unix: Some(now - 21_600),
        unanswered_since_unix: None,
    };
    assert_eq!(due(now, &silent, &defaults), vec![Due::Checkin, Due::Probe]);
    let recent = Watch {
        last_seen_unix: Some(now - 60),
        unanswered_since_unix: None,
    };
    assert!(due(now, &recent, &defaults).is_empty());
    // A subordinate watches upward too: team-b's peers are its lead only.
    assert_eq!(
        topology.watched_peers("team-b@host-2"),
        vec!["team-a@host-1".to_string()]
    );
}

#[test]
fn unanswered_message_triggers_probe_at_sixty_minutes() {
    let topology = merged();
    let defaults = topology.defaults_for("team-b");
    let now = 1_000_000;
    let at_sixty = Watch {
        last_seen_unix: Some(now - 60),
        unanswered_since_unix: Some(now - 3_600),
    };
    assert_eq!(due(now, &at_sixty, &defaults), vec![Due::Probe]);
    let at_fiftynine = Watch {
        last_seen_unix: Some(now - 60),
        unanswered_since_unix: Some(now - 3_599),
    };
    assert!(due(now, &at_fiftynine, &defaults).is_empty());
}

#[test]
fn unhealthy_peer_kicked_exactly_once_per_incident() {
    assert!(should_kick(PeerHealth::Unhealthy));
    assert!(should_kick(PeerHealth::LeadUnreachable));
    assert!(!should_kick(PeerHealth::Healthy));
    // Deliberately held (e.g. a turnover pin) is not unhealthy.
    assert!(!should_kick(PeerHealth::Held));

    // Several probers share one per-host ledger through its JSON form.
    let mut first = IncidentLedger::default();
    assert!(first.claim_kick("team-a@host-1", "incident-1"));
    let mut second = IncidentLedger::from_json(&first.to_json()).expect("ledger reloads");
    assert!(!second.claim_kick("team-a@host-1", "incident-1"));
    assert!(!first.claim_kick("team-a@host-1", "incident-1"));
    // A new incident, and a different peer, may each be kicked once.
    assert!(second.claim_kick("team-a@host-1", "incident-2"));
    assert!(second.claim_kick("team-b@host-2", "incident-1"));
}

#[test]
fn conflicting_declarations_are_reported_never_merged() {
    let a = parse_fragment(TEAM_A, "team-a").expect("team-a parses");
    let duplicate = parse_fragment(TEAM_A, "team-a").expect("team-a parses again");
    let conflicts = merge(vec![a.clone(), duplicate], None).expect_err("duplicate must conflict");
    assert!(conflicts.iter().any(|c| c.contains("declared twice")));

    let b = parse_fragment(TEAM_B, "team-b").expect("team-b parses");
    let conflicts = merge(vec![a, b], None).expect("consistent pair merges");
    assert_eq!(conflicts.instances.len(), 2);

    // Lead/subordinate mismatch: team-b stops naming its lead.
    let drifting = TEAM_B.replace("\"team-a@host-1\"", "null");
    let b2 = parse_fragment(&drifting, "team-b").expect("drifting team-b parses");
    let a2 = parse_fragment(TEAM_A, "team-a").expect("team-a parses");
    let conflicts = merge(vec![a2, b2], None).expect_err("mismatch must conflict");
    assert!(conflicts.iter().any(|c| c.contains("subordinate")));
}

#[test]
fn defaults_are_the_watchers_own_policy() {
    // team-b sets a shorter check-in in its own fragment: not a conflict.
    let eager = TEAM_B.replace(
        "\"instances\"",
        "\"checkin_after_silence_seconds\": 60, \"instances\"",
    );
    let a = parse_fragment(TEAM_A, "team-a").expect("team-a parses");
    let b = parse_fragment(&eager, "team-b").expect("eager team-b parses");
    let topology = merge(vec![a, b], None).expect("different defaults merge");
    assert_eq!(
        topology
            .defaults_for("team-b")
            .checkin_after_silence_seconds,
        60
    );
    assert_eq!(
        topology
            .defaults_for("team-a")
            .checkin_after_silence_seconds,
        21_600
    );
    // A team never inherits another team's fragment values.
    assert_eq!(
        topology
            .defaults_for("team-b")
            .probe_after_unanswered_seconds,
        3_600
    );
}

#[test]
fn tick_kicks_unhealthy_peer_once_and_notifies_once() {
    let topology = merged();
    let defaults = topology.defaults_for("team-a");
    let now = 1_000_000;
    let silent = Watch {
        last_seen_unix: Some(now - 21_600),
        unanswered_since_unix: None,
    };
    let mut io = FakeIo {
        snapshot: Some(DriverSnapshot {
            state: "ESCALATE".to_string(),
            incident: "incident-9".to_string(),
            held: false,
        }),
        ..FakeIo::default()
    };
    let mut ledger = IncidentLedger::default();
    let first = tick(
        now,
        "team-b@host-2",
        &silent,
        &defaults,
        &mut io,
        &mut ledger,
    );
    assert_eq!(
        first,
        vec![
            Event::CheckinSent,
            Event::Probed,
            Event::Kicked,
            Event::Notified
        ]
    );
    // A second prober sharing the ledger kicks and notifies no more.
    let mut other = IncidentLedger::from_json(&ledger.to_json()).expect("ledger reloads");
    let second = tick(
        now,
        "team-b@host-2",
        &silent,
        &defaults,
        &mut io,
        &mut other,
    );
    assert_eq!(second, vec![Event::CheckinSent, Event::Probed]);
    assert_eq!(io.kicks, 1);
    assert_eq!(io.notifies, 1);
}

#[test]
fn tick_never_kicks_a_held_or_healthy_peer() {
    let topology = merged();
    let defaults = topology.defaults_for("team-a");
    let now = 1_000_000;
    let silent = Watch {
        last_seen_unix: Some(now - 21_600),
        unanswered_since_unix: None,
    };
    for snapshot in [
        DriverSnapshot {
            state: "HEALTHY".to_string(),
            incident: "incident-1".to_string(),
            held: false,
        },
        DriverSnapshot {
            state: "HANDOFF".to_string(),
            incident: "incident-1".to_string(),
            held: true,
        },
    ] {
        let mut io = FakeIo {
            snapshot: Some(snapshot),
            ..FakeIo::default()
        };
        let mut ledger = IncidentLedger::default();
        let events = tick(
            now,
            "team-b@host-2",
            &silent,
            &defaults,
            &mut io,
            &mut ledger,
        );
        assert_eq!(events, vec![Event::CheckinSent, Event::Probed]);
        assert_eq!(io.kicks, 0);
    }
}

#[test]
fn tick_kicks_an_unreachable_peer_once_per_silence() {
    let topology = merged();
    let defaults = topology.defaults_for("team-a");
    let now = 1_000_000;
    let silent = Watch {
        last_seen_unix: Some(now - 21_600),
        unanswered_since_unix: None,
    };
    let mut io = FakeIo::default();
    let mut ledger = IncidentLedger::default();
    let first = tick(
        now,
        "team-b@host-2",
        &silent,
        &defaults,
        &mut io,
        &mut ledger,
    );
    assert_eq!(
        first,
        vec![Event::CheckinSent, Event::Kicked, Event::Notified]
    );
    let second = tick(
        now,
        "team-b@host-2",
        &silent,
        &defaults,
        &mut io,
        &mut ledger,
    );
    assert_eq!(second, vec![Event::CheckinSent]);
    assert_eq!(io.kicks, 1);
}

#[test]
fn routes_override_must_name_declared_routes() {
    let with_override = TEAM_A.replace(
        "\"instances\"",
        "\"routes_override\": {\"host-1->host-2\": {\"kick\": \"remote-exec-2\"}}, \"instances\"",
    );
    let a = parse_fragment(&with_override, "team-a").expect("override fragment parses");
    let b = parse_fragment(TEAM_B, "team-b").expect("team-b parses");
    let topology = merge(vec![a, b], None).expect("fragments merge");
    let declared = |host: &str| -> BTreeSet<String> {
        if host == "host-2" {
            BTreeSet::from(["local".to_string(), "remote-exec-2".to_string()])
        } else {
            BTreeSet::from(["local".to_string()])
        }
    };
    assert!(validate_routes(&topology, &declared).is_empty());
    let none_declared = |_: &str| -> BTreeSet<String> { BTreeSet::new() };
    let problems = validate_routes(&topology, &none_declared);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("remote-exec-2"));
}
