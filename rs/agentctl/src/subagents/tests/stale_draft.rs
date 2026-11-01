//! #720 agentctl-muse-stale-draft: a prompt agentctl typed that Muse leaves unsent in its editor
//! no longer holds every later prompt, and a draft agentctl did not type is never submitted.
use super::herdr_compatibility::{Clock, MuseEditor};
use super::*;

const LANE: &str = "[from tickhub-ops-2 via agentctl · reply: agentctl send tickhub-ops-2]";
const OWNER: &str = "[from Ada via slack #team · sent: 2026.11.01:02:46 EDT · reply: <r7>]";

fn start_muse(fixture: &Fixture, busy: bool) -> ManagedAgents<'_, Fake> {
    let manager = fixture.manager();
    manager
        .start(
            "coord",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness: "muse".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let status = if busy { "working" } else { "idle" };
    *fixture.client.status.lock().unwrap() = Some(status.to_owned());
    let mut editor = MuseEditor::new("YOLO", false, None);
    editor.busy = busy;
    *fixture.client.muse_editor.lock().unwrap() = Some(editor);
    manager
}

fn quick() -> DrainOptions {
    DrainOptions {
        ready_timeout: Duration::ZERO,
        working_timeout: Duration::from_secs(5),
        ..DrainOptions::default()
    }
}

fn keys(fixture: &Fixture) -> Vec<String> {
    fixture
        .client
        .keys_sent
        .lock()
        .unwrap()
        .iter()
        .map(|(_, key)| key.clone())
        .collect()
}

fn editor<T>(fixture: &Fixture, look: impl FnOnce(&mut MuseEditor) -> T) -> T {
    look(fixture.client.muse_editor.lock().unwrap().as_mut().unwrap())
}

fn processed(manager: &ManagedAgents<'_, Fake>, id: &str) -> Value {
    agent::read_private_json(
        &manager
            .queue("coord")
            .unwrap()
            .join(format!("processed/{id}.json")),
    )
    .unwrap()
}

#[test]
fn a_steer_muse_shows_and_then_refuses_is_pressed_again_not_counted_delivered() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture, true);
    editor(&fixture, |editor| editor.failing_steers = 1);
    let prompt = format!("{LANE}\nvalidate is LAUNCHED but QUEUED");
    let result = manager
        .send_identified_with_runtime("coord", &prompt, quick(), "lane", &Clock::default())
        .unwrap();
    assert_eq!(result.delivered, ["lane"]);
    // The first steer showed as a user turn, then came back to the editor; agentctl pressed
    // Enter again instead of counting the screen's first proof.
    assert_eq!(keys(&fixture), ["Enter", "Enter"]);
    let screen = editor(&fixture, |editor| {
        assert!(editor.staged().is_none(), "the editor is empty");
        editor.screen()
    });
    assert!(screen.contains("! steer failed"), "{screen}");
    assert!(
        screen.contains("validate is LAUNCHED but QUEUED"),
        "{screen}"
    );
    assert_eq!(processed(&manager, "lane")["delivery_state"], "processed");
}

/// The 2026-11-01 deadlock: an agent's message was counted delivered, then Muse put it back in
/// the editor, and every later delivery, the owner's included, was refused as "a draft". Now the
/// next delivery submits the draft agentctl typed and carries on, with no human action.
#[test]
fn a_draft_agentctl_typed_is_submitted_by_the_next_delivery_with_no_human_action() {
    for busy in [true, false] {
        let fixture = Fixture::new();
        let manager = start_muse(&fixture, busy);
        let lane = format!("{LANE}\nvalidate at 87daeb34 is LAUNCHED but QUEUED");
        manager
            .send_identified_with_runtime("coord", &lane, quick(), "lane", &Clock::default())
            .unwrap();
        // After agentctl's proof, Muse leaves the prompt unsent in its editor.
        editor(&fixture, |editor| editor.draft(&lane));
        if !busy {
            assert_eq!(
                manager.status("coord").unwrap()["agent_status"],
                "staged",
                "an idle Muse with a draft reads staged"
            );
        }
        fixture.client.keys_sent.lock().unwrap().clear();

        let owner = format!("{OWNER}\nare you there?");
        let result = manager
            .send_identified_with_runtime("coord", &owner, quick(), "chat-7", &Clock::default())
            .unwrap();
        assert_eq!(result.delivered, ["chat-7"], "busy={busy}");
        // One Enter submits the stale draft, one the owner's message.
        assert_eq!(keys(&fixture), ["Enter", "Enter"], "busy={busy}");
        let screen = editor(&fixture, |editor| {
            assert!(editor.staged().is_none(), "busy={busy}");
            editor.screen()
        });
        assert!(screen.contains("are you there?"), "busy={busy}: {screen}");
        let recovered = processed(&manager, "lane");
        assert!(recovered["draft_recovered_at"].is_number(), "{recovered}");
        assert!(
            recovered["draft_recovered_evidence"]
                .as_str()
                .unwrap()
                .contains("typed by agentctl for tickhub-ops-2"),
            "{recovered}"
        );
        assert_eq!(processed(&manager, "chat-7")["delivery_state"], "processed");
    }
}

#[test]
fn a_draft_agentctl_did_not_type_is_refused_and_reported_never_submitted() {
    for busy in [true, false] {
        let fixture = Fixture::new();
        let manager = start_muse(&fixture, busy);
        // An earlier prompt agentctl typed, so the queue has records to compare with.
        manager
            .send_identified_with_runtime(
                "coord",
                &format!("{LANE}\nearlier"),
                quick(),
                "earlier",
                &Clock::default(),
            )
            .unwrap();
        fixture.client.keys_sent.lock().unwrap().clear();
        fixture.client.literal_pastes.lock().unwrap().clear();
        editor(&fixture, |editor| {
            editor.draft("half-typed note from a person")
        });
        let error = manager
            .send_identified_with_runtime(
                "coord",
                &format!("{OWNER}\nhello"),
                quick(),
                "chat-8",
                &Clock::default(),
            )
            .unwrap_err();
        assert!(
            matches!(error, AgentError::Pending(_)),
            "busy={busy}: {error}"
        );
        let text = error.to_string();
        assert!(
            !text.contains("half-typed"),
            "the draft is never quoted: {text}"
        );
        if busy {
            assert!(
                text.contains(
                    "holds a draft that agentctl did not type (29 characters on 1 row(s))"
                ),
                "{text}"
            );
            assert!(text.contains("a person may be typing it"), "{text}");
        } else {
            // An idle pane is refused as `staged`, worded as every edition words it.
            assert!(
                text.ends_with("did not become idle/done within 0s; last status=staged"),
                "{text}"
            );
        }
        // Nothing was pressed or typed, and the person's draft is still there.
        assert!(
            keys(&fixture).is_empty(),
            "busy={busy}: {:?}",
            keys(&fixture)
        );
        assert!(fixture.client.literal_pastes.lock().unwrap().is_empty());
        assert_eq!(
            editor(&fixture, |editor| editor.staged().map(str::to_owned)),
            Some("half-typed note from a person".to_owned())
        );
        let retained = agent::read_private_json(&error.undelivered().unwrap().artifact).unwrap();
        assert_eq!(retained["delivery_state"], "pending");
        let reason = retained["delivery_error"].as_str().unwrap();
        assert!(
            reason.contains(if busy {
                "did not type"
            } else {
                "last status=staged"
            }),
            "{reason}"
        );
    }
}

#[test]
fn a_prompt_an_idle_muse_missed_the_enter_for_is_pressed_again() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture, false);
    editor(&fixture, |editor| editor.missed_enters = 1);
    let prompt = format!("{LANE}\nstatus?");
    let result = manager
        .send_identified_with_runtime("coord", &prompt, quick(), "missed", &Clock::default())
        .unwrap();
    assert_eq!(result.delivered, ["missed"]);
    assert_eq!(keys(&fixture), ["Enter", "Enter"]);
    assert!(editor(&fixture, |editor| editor.staged().is_none()));
}

#[test]
fn a_batch_left_unsent_is_found_by_the_text_the_drain_typed() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture, true);
    let queue = manager.queue("coord").unwrap();
    let combined = format!("2 queued messages:\n\n{LANE}\none\n\n{LANE}\ntwo");
    for id in ["b1", "b2"] {
        let directory = queue.join("processed");
        if !directory.is_dir() {
            fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(&directory)
                .unwrap();
        }
        agent::atomic_json(
            &directory.join(format!("{id}.json")),
            &json!({"id": id, "text": format!("{LANE}\n{id}"), "typed_as": combined,
                    "delivery_state": "processed", "inflight_at": 5.0}),
        )
        .unwrap();
    }
    editor(&fixture, |editor| editor.draft(&combined));
    let result = manager
        .send_identified_with_runtime(
            "coord",
            &format!("{OWNER}\nnext"),
            quick(),
            "chat-9",
            &Clock::default(),
        )
        .unwrap();
    assert_eq!(result.delivered, ["chat-9"]);
    assert_eq!(keys(&fixture), ["Enter", "Enter"]);
}

/// The recovered draft's steer can fail again (recovery happens just when steers fail): it is
/// pressed again before the next prompt is pasted, so the paste never meets it in the editor.
#[test]
fn a_recovered_draft_whose_steer_fails_again_is_pressed_again_before_the_next_paste() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture, true);
    let lane = format!("{LANE}\nthe lock is released");
    manager
        .send_identified_with_runtime("coord", &lane, quick(), "lane", &Clock::default())
        .unwrap();
    editor(&fixture, |editor| {
        editor.draft(&lane);
        editor.failing_steers = 1;
    });
    fixture.client.keys_sent.lock().unwrap().clear();
    let owner = format!("{OWNER}\nstatus?");
    let result = manager
        .send_identified_with_runtime("coord", &owner, quick(), "chat-3", &Clock::default())
        .unwrap();
    assert_eq!(result.delivered, ["chat-3"]);
    // Enter for the draft, Enter again once its steer failed and brought it back, then Enter
    // for the owner's prompt.
    assert_eq!(keys(&fixture), ["Enter", "Enter", "Enter"]);
    let screen = editor(&fixture, |editor| {
        assert!(editor.staged().is_none());
        editor.screen()
    });
    assert!(screen.contains("! steer failed"), "{screen}");
    assert!(screen.contains("❯ [from tickhub-ops-2"), "{screen}");
    assert!(screen.contains("status?"), "{screen}");
}
