//! #505 agentctl-muse-followup: a queue drain types an ordinary prompt into a Muse pane whose
//! turn is running, and Muse steers the turn with it.
use super::herdr_compatibility::{Clock, MuseEditor};
use super::*;

fn start_busy_muse(fixture: &Fixture, header: bool) -> ManagedAgents<'_, Fake> {
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness: "muse".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    *fixture.client.status.lock().unwrap() = Some("working".to_owned());
    let mut editor = MuseEditor::new("Launch overrides", header, None);
    editor.busy = true;
    *fixture.client.muse_editor.lock().unwrap() = Some(editor);
    manager
}

fn held() -> DrainOptions {
    DrainOptions {
        ready_timeout: Duration::ZERO,
        working_timeout: Duration::from_millis(50),
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

fn no_input(fixture: &Fixture) {
    assert!(fixture.client.literal_pastes.lock().unwrap().is_empty());
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
    assert!(fixture.client.runs.lock().unwrap().is_empty());
}

#[test]
fn a_working_muse_takes_an_ordinary_prompt_by_steering_its_turn() {
    for header in [false, true] {
        let fixture = Fixture::new();
        let manager = start_busy_muse(&fixture, header);
        assert_eq!(manager.status("worker").unwrap()["agent_status"], "working");
        let prompt = "hold: stop after the current step\nand report";
        let result = manager
            .send_identified_with_runtime("worker", prompt, held(), "hold", &Clock::default())
            .unwrap();
        assert_eq!(result.delivered, ["hold"], "header={header}");
        assert_eq!(
            *fixture.client.literal_pastes.lock().unwrap(),
            [(
                "owned".to_owned(),
                format!("{BRACKETED_PASTE_START}{prompt}{BRACKETED_PASTE_END}")
            )]
        );
        // One Enter, and no key that could interrupt the turn.
        assert_eq!(keys(&fixture), ["Enter"]);
        let editor = fixture.client.muse_editor.lock().unwrap();
        let screen = editor.as_ref().unwrap().screen();
        assert!(screen.contains(&format!("❯ {prompt}")));
        assert!(screen.contains("esc to interrupt"), "the turn kept running");
        drop(editor);
        let processed = manager.queue("worker").unwrap().join("processed/hold.json");
        assert_eq!(
            agent::read_private_json(&processed).unwrap()["delivery_state"],
            "processed"
        );
    }
}

#[test]
fn a_watchers_drain_types_a_queued_prompt_into_a_busy_muse() {
    // #804 live finding: queue-only prompts sat pending in working Muse lanes
    // while the watcher reported typed 0 with no error. The shared drain the
    // watchers run must steer one in, exactly as a send does.
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, false);
    manager
        .queue_identified("worker", "queued while busy", None, Some("watch-queued"))
        .unwrap();
    let rows = manager.drain_pending(
        std::time::Duration::from_secs(600),
        false,
        crate::agent::now_millis(),
    );
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["name"], "worker");
    assert_eq!(rows[0]["typed"], 1, "{rows:?}");
    assert_eq!(rows[0]["remaining"], 0, "{rows:?}");
    assert_eq!(rows[0]["stale"], false, "{rows:?}");
    assert!(
        rows[0]["oldest_seconds"]
            .as_u64()
            .is_some_and(|age| age < 60),
        "age is an age, not an epoch: {rows:?}"
    );
}

#[test]
fn a_follow_up_muse_never_accepts_is_taken_back_out_and_stays_pending() {
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, false);
    fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .ignore_enter = true;
    let error = manager
        .send_identified_with_runtime("worker", "never taken", held(), "never", &Clock::default())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert_eq!(error.exit_code(), 75);
    assert!(error.to_string().contains("taken back out"), "{error}");
    let sent = keys(&fixture);
    // Enter is pressed again only while the exact prompt is still staged, then the clearing keys
    // take it out; Esc, which interrupts a Muse turn, is never sent.
    let enters = sent.iter().take_while(|key| *key == "Enter").count();
    assert!(enters > 1, "{sent:?}");
    assert_eq!(&sent[enters..], ["ctrl+e", "ctrl+u"]);
    assert_eq!(fixture.client.literal_pastes.lock().unwrap().len(), 1);
    assert!(fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .staged()
        .is_none());
    let retained = agent::read_private_json(&error.undelivered().unwrap().artifact).unwrap();
    assert_eq!(retained["delivery_state"], "pending");
    assert!(retained.get("possibly_submitted").is_none());
    assert!(!manager
        .queue("worker")
        .unwrap()
        .join("failed/never.json")
        .exists());
}

#[test]
fn an_operator_draft_an_approval_form_or_a_slash_command_holds_the_prompt_untyped() {
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, false);
    let set = |change: &dyn Fn(&mut MuseEditor)| {
        change(fixture.client.muse_editor.lock().unwrap().as_mut().unwrap());
    };

    set(&|editor| editor.draft("owner draft"));
    let error = manager
        .send("worker", "behind a draft", held())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(error.to_string().contains("nothing was typed"), "{error}");
    no_input(&fixture);
    fs::remove_file(&error.undelivered().unwrap().artifact).unwrap();

    set(&|editor| {
        editor.key("ctrl+u");
        editor.approval = true;
    });
    let error = manager
        .send("worker", "at an approval", held())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    // The approval form waits for a person, so the pane reads blocked, not working (#678).
    assert!(error.to_string().contains("is blocked"), "{error}");
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "blocked");
    no_input(&fixture);
    fs::remove_file(&error.undelivered().unwrap().artifact).unwrap();

    // A peer-approval dialog also holds the prompt untyped (#974): whether a peer may
    // talk to the session is a human decision, so send refuses and types nothing.
    set(&|editor| {
        editor.approval = false;
        editor.peer_approval = true;
    });
    let error = manager
        .send("worker", "at a peer dialog", held())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(error.to_string().contains("is blocked"), "{error}");
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "blocked");
    no_input(&fixture);
    fs::remove_file(&error.undelivered().unwrap().artifact).unwrap();
    set(&|editor| editor.peer_approval = false);

    // Slash commands open Muse menus at once even mid-turn, so they wait for an idle editor.
    set(&|editor| editor.approval = false);
    let error = manager.send("worker", "/model", held()).unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(error.to_string().contains("last status=working"), "{error}");
    no_input(&fixture);
}

// Owner-directed: an adopted Muse pane takes a follow-up at whatever footer mode it shows, so an
// adopted coordinator running in YOLO still receives messages while it works.
#[test]
fn an_adopted_working_muse_takes_follow_ups_at_any_footer_mode() {
    for mode in ["YOLO", "", "Launch overrides", "Auto-review"] {
        let fixture = Fixture::new();
        fixture.prepare_foreign();
        fixture
            .client
            .custom_reported
            .store(true, Ordering::Relaxed);
        *fixture.client.status.lock().unwrap() = Some("idle".to_owned());
        *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::new(mode, true, None));
        let mut options = fixture.adopt_options();
        options.harness = "muse".to_owned();
        let manager = fixture.manager();
        manager.adopt("foreign", options).unwrap();
        *fixture.client.status.lock().unwrap() = Some("working".to_owned());
        fixture
            .client
            .muse_editor
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .busy = true;
        assert_eq!(
            manager.status("foreign").unwrap()["agent_status"],
            "working",
            "{mode:?}"
        );
        let result = manager
            .send_identified_with_runtime(
                "foreign",
                "steer the adopted turn",
                held(),
                "steer",
                &Clock::default(),
            )
            .unwrap();
        assert_eq!(result.delivered, ["steer"], "{mode:?}");
        assert_eq!(keys(&fixture), ["Enter"], "{mode:?}");
        let editor = fixture.client.muse_editor.lock().unwrap();
        let screen = editor.as_ref().unwrap().screen();
        assert!(screen.contains("❯ steer the adopted turn"), "{mode:?}");
        assert!(
            screen.contains("esc to interrupt"),
            "{mode:?}: the turn kept running"
        );
    }
}

#[test]
fn an_adopted_muse_whose_pane_reports_no_agent_still_takes_working_follow_ups() {
    // #697 send-muse-while-working: Herdr has no Muse manifest (#691), so an
    // adopted Muse pane can report no agent kind at all, and once the harness
    // re-execs the pinned-process correction no longer names it either. The
    // record's harness must still drive readiness, or a busy adopted Muse
    // waits for idle like the other team's coordinator did.
    let fixture = Fixture::new();
    fixture.prepare_foreign();
    fixture
        .client
        .custom_reported
        .store(true, Ordering::Relaxed);
    *fixture.client.status.lock().unwrap() = Some("idle".to_owned());
    *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::new("YOLO", true, None));
    let mut options = fixture.adopt_options();
    options.harness = "muse".to_owned();
    let manager = fixture.manager();
    manager.adopt("foreign", options).unwrap();
    let record_path = fixture.root.join("registry/agents/foreign/agent.json");
    let mut record = agent::read_private_json(&record_path).unwrap();
    record
        .as_object_mut()
        .unwrap()
        .insert("anchor_rule".to_owned(), serde_json::Value::Null);
    agent::atomic_json(&record_path, &record).unwrap();
    *fixture.client.status.lock().unwrap() = Some("working".to_owned());
    fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .busy = true;
    // Readiness now reaches the typing stage instead of waiting out the ready
    // timeout for idle; typing itself still refuses without a process anchor,
    // pending with nothing typed. (The Fake cannot express a re-exec'd harness
    // whose shell identity verifies while the foreground pin does not.)
    let error = manager
        .send_identified_with_runtime(
            "foreign",
            "steer without a reported kind",
            held(),
            "steer",
            &Clock::default(),
        )
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("foreground harness process anchor"),
        "{error}"
    );
    assert!(
        !error.to_string().contains("did not become idle"),
        "{error}"
    );
    no_input(&fixture);
}

#[test]
fn a_revived_adopted_muse_on_the_native_adapter_still_verifies_follow_ups() {
    // #697 send-muse-while-working: revive re-owns an adopted pane under adapter `herdr`;
    // a Muse record there must keep the verified-composer path or busy sends wait.
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, true);
    let mut record = manager.load("worker").unwrap();
    record.adapter = "herdr".to_owned();
    let client = manager.workspace_client(&record);
    assert!(agent::AgentApi::verifies_muse_followups(&client));
}

#[test]
fn a_paste_never_seen_as_the_exact_editor_is_taken_back_out_and_quarantined() {
    // Idle and running turns alike: no Enter is sent, and the paste is cleared out of the editor
    // rather than left there unsubmitted. The prompt is quarantined as possibly submitted, so it
    // is neither typed again on every drain nor holds the queue.
    for busy in [false, true] {
        let fixture = Fixture::new();
        let manager = start_busy_muse(&fixture, true);
        {
            let mut editor = fixture.client.muse_editor.lock().unwrap();
            let editor = editor.as_mut().unwrap();
            editor.busy = busy;
            editor.garble = true;
        }
        if !busy {
            *fixture.client.status.lock().unwrap() = Some("done".to_owned());
        }
        let error = manager
            .send_identified_with_runtime(
                "worker",
                "never exact",
                held(),
                "garbled",
                &Clock::default(),
            )
            .unwrap_err();
        assert_eq!(error.exit_code(), 76, "busy={busy}: {error}");
        assert!(error.to_string().contains("taken back out"), "{error}");
        let sent = keys(&fixture);
        assert!(
            !sent.iter().any(|key| key == "Enter" || key == "esc"),
            "{sent:?}"
        );
        assert!(sent.iter().any(|key| key == "ctrl+u"), "{sent:?}");
        assert!(fixture
            .client
            .muse_editor
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .staged()
            .is_none());
        let queue = manager.queue("worker").unwrap();
        assert!(queue.join("failed/garbled.json").is_file());
        assert!(!queue.join("inbox/garbled.json").exists());
    }
}

// #1085 muse-send-staged-unsubmitted: a Muse that stops reading its terminal draws a paste late,
// or only its start. Keys sent into it then act later on whatever the editor holds: in the
// incident a clear cut one prompt down to its sender header and the next prompt was glued on.

fn stall(fixture: &Fixture, busy: bool, drawn: usize) {
    {
        let mut editor = fixture.client.muse_editor.lock().unwrap();
        let editor = editor.as_mut().unwrap();
        editor.busy = busy;
        editor.stalled = Some(drawn);
    }
    if !busy {
        *fixture.client.status.lock().unwrap() = Some("done".to_owned());
    }
}

const STALLED_PROMPT: &str = "[from coordinator via agentctl · reply: agentctl send coordinator]\n\
                              Stage C hold state accepted: hold both heads byte-exact.";

#[test]
fn a_paste_muse_has_not_drawn_is_left_untouched_and_reported_not_delivered() {
    // Nothing drawn, or only the sender header and the start of the body; idle and busy alike.
    for (busy, drawn) in [(true, 0), (true, 90), (false, 0), (false, 90)] {
        let fixture = Fixture::new();
        let manager = start_busy_muse(&fixture, false);
        stall(&fixture, busy, drawn);
        let error = manager
            .send_identified_with_runtime(
                "worker",
                STALLED_PROMPT,
                held(),
                "stalled",
                &Clock::default(),
            )
            .unwrap_err();
        let case = format!("busy={busy} drawn={drawn}: {error}");
        // Pending (exit 75), never possibly submitted, and the sender is told why.
        assert!(matches!(error, AgentError::Pending(_)), "{case}");
        assert_eq!(error.exit_code(), 75, "{case}");
        let text = error.to_string();
        assert!(text.contains("input_stalled:"), "{case}");
        assert!(text.contains("NOT delivered"), "{case}");
        assert!(text.contains("left untouched"), "{case}");
        // One paste and not a single key: no Enter, no clearing key.
        assert_eq!(
            fixture.client.literal_pastes.lock().unwrap().len(),
            1,
            "{case}"
        );
        assert!(keys(&fixture).is_empty(), "{case}: {:?}", keys(&fixture));
        let queue = manager.queue("worker").unwrap();
        let pending = agent::read_private_json(&queue.join("inbox/stalled.json")).unwrap();
        assert!(pending.get("stranded_at").is_some(), "{case}");
        assert!(!queue.join("failed/stalled.json").exists(), "{case}");
        // Status names the stalled prompt.
        let status = manager.status("worker").unwrap();
        assert_eq!(
            status["input_stalled"][0]["message_id"], "stalled",
            "{case}: {status}"
        );
    }
}

#[test]
fn a_later_send_to_an_input_stalled_pane_queues_untyped_and_both_land_once_muse_reads_again() {
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, false);
    stall(&fixture, true, 90);
    manager
        .send_identified_with_runtime(
            "worker",
            STALLED_PROMPT,
            held(),
            "stalled",
            &Clock::default(),
        )
        .unwrap_err();
    // An owner prompt, which otherwise goes first, is not pasted on top of the stalled paste:
    // the stranded prompt is looked at first, and the owner's waits, untyped.
    let owner =
        "[from human@devbig via agentctl · reply: agentctl send human@devbig]\nowner: status?";
    let error = manager
        .send_identified_with_runtime("worker", owner, held(), "owner", &Clock::default())
        .unwrap_err();
    assert_eq!(error.exit_code(), 75, "{error}");
    assert_eq!(fixture.client.literal_pastes.lock().unwrap().len(), 1);
    assert!(keys(&fixture).is_empty(), "{:?}", keys(&fixture));
    let queue = manager.queue("worker").unwrap();
    assert!(queue.join("inbox/owner.json").is_file());
    // An urgent send, which types only its own prompt, waits too, and says why; no turn is
    // interrupted.
    let urgent = manager
        .send_urgent(
            "worker",
            "urgent: stop",
            held(),
            Some("urgent"),
            Duration::ZERO,
        )
        .unwrap_err();
    assert_eq!(urgent.exit_code(), 75, "{urgent}");
    assert!(
        urgent.to_string().contains("pane input stalled"),
        "{urgent}"
    );
    assert_eq!(fixture.client.literal_pastes.lock().unwrap().len(), 1);
    assert!(keys(&fixture).is_empty(), "{:?}", keys(&fixture));
    let waiting = agent::read_private_json(&queue.join("inbox/urgent.json")).unwrap();
    let why = waiting["delivery_error"].as_str().unwrap();
    assert!(why.contains("input_stalled:"), "{why}");
    assert!(why.contains("queued behind message stalled"), "{why}");
    // Still stalled on the next pass: the stranded prompt is never typed again.
    let error = manager
        .send_identified_with_runtime("worker", "third", held(), "third", &Clock::default())
        .unwrap_err();
    assert_eq!(error.exit_code(), 75, "{error}");
    assert_eq!(fixture.client.literal_pastes.lock().unwrap().len(), 1);
    assert!(keys(&fixture).is_empty(), "{:?}", keys(&fixture));

    // Muse reads its terminal again and draws the whole paste. The next pass presses Enter on
    // it once, then types the prompts that waited.
    let submitted = fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .release();
    assert!(submitted.is_empty(), "no key was held to submit it");
    let result = manager
        .send_identified_with_runtime("worker", "fourth", held(), "fourth", &Clock::default())
        .unwrap();
    for id in ["stalled", "owner", "urgent", "third", "fourth"] {
        assert!(
            queue.join(format!("processed/{id}.json")).is_file(),
            "{id}: {result:?}"
        );
    }
    let pastes = fixture.client.literal_pastes.lock().unwrap();
    assert_eq!(pastes.len(), 5, "the stalled prompt was pasted once");
    assert_eq!(
        pastes
            .iter()
            .filter(|(_, text)| text.contains("Stage C hold state accepted"))
            .count(),
        1
    );
    drop(pastes);
    assert_eq!(keys(&fixture), ["Enter"; 5], "one Enter per prompt");
    let screen = fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .screen();
    assert_eq!(
        screen.matches("Stage C hold state accepted").count(),
        1,
        "{screen}"
    );
}

/// The #1323 notice, as Muse draws a paste of it: wrapped long lines, `❯` on the first
/// row, a two-space continuation indent, the status footer below the bottom rule.
const WRAPPED_NOTICE: &str = "[from coordinator via agentctl · reply: agentctl send coordinator]\n\
                              coordinator -> all lanes on this host: the data disk is low and \
                              another team's builds are frozen. Until further notice:\n\
                              (1) Run at most ONE local validate or build at a time per lane, \
                              and never in parallel nested worktrees.\n\
                              (2) As soon as a validate or gate finishes (never mid-validate), \
                              delete the regenerable build output it left in your slot or \
                              nested worktrees: cargo target/ dirs (rs/target, vibe-talk/target, \
                              etc.) and page build trees. If your next validate would rebuild \
                              them anyway, you may keep the ones in your MAIN slot checkout, but \
                              delete extra nested-worktree copies.\n\
                              No reply needed; just do it at your next safe point.";

#[test]
fn a_wrapped_notice_drawn_in_the_editor_is_submitted_exactly_once_idle_and_busy() {
    // #1323 muse-drawn-prompt-missed: the drawn check must undo the wrapping, the `❯`
    // prefix and the continuation indent, for an idle and for a busy Muse alike, and
    // press Enter once — never strand the prompt as not drawn.
    for busy in [false, true] {
        let fixture = Fixture::new();
        let manager = start_busy_muse(&fixture, false);
        {
            let mut editor = fixture.client.muse_editor.lock().unwrap();
            let editor = editor.as_mut().unwrap();
            editor.busy = busy;
            editor.wrap_columns = Some(80);
        }
        if !busy {
            *fixture.client.status.lock().unwrap() = Some("done".to_owned());
        }
        let result = manager
            .send_identified_with_runtime(
                "worker",
                WRAPPED_NOTICE,
                held(),
                "wrapped",
                &Clock::default(),
            )
            .unwrap();
        assert_eq!(result.delivered, ["wrapped"], "busy={busy}");
        assert_eq!(keys(&fixture), ["Enter"], "busy={busy}");
        assert_eq!(
            fixture.client.literal_pastes.lock().unwrap().len(),
            1,
            "busy={busy}"
        );
        let queue = manager.queue("worker").unwrap();
        assert!(
            queue.join("processed/wrapped.json").is_file(),
            "busy={busy}"
        );
        assert!(!queue.join("inbox/wrapped.json").exists(), "busy={busy}");
    }
}

#[test]
fn a_stranded_paste_that_never_draws_is_reported_lost_and_releases_the_queue() {
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, false);
    stall(&fixture, true, 0);
    manager
        .send_identified_with_runtime(
            "worker",
            STALLED_PROMPT,
            held(),
            "stalled",
            &Clock::default(),
        )
        .unwrap_err();
    // Backdate the strand past the late-paste window.
    let queue = manager.queue("worker").unwrap();
    let path = queue.join("inbox/stalled.json");
    let mut document = agent::read_private_json(&path).unwrap();
    document["stranded_at"] = json!(1.0);
    agent::atomic_json(&path, &document).unwrap();
    // The next pass reports the paste lost; it no longer holds the queue, so the next prompt is
    // typed (and, Muse being still stalled, stranded in its turn).
    manager
        .send_identified_with_runtime("worker", "next", held(), "next", &Clock::default())
        .unwrap_err();
    let document = agent::read_private_json(&path).unwrap();
    assert!(document["delivery_error"]
        .as_str()
        .unwrap()
        .starts_with("late_paste_lost:"));
    let pastes = fixture.client.literal_pastes.lock().unwrap();
    assert_eq!(pastes.len(), 2, "the lost prompt is never typed again");
    assert!(pastes[1].1.contains("next"));
    drop(pastes);
    assert!(keys(&fixture).is_empty(), "{:?}", keys(&fixture));
    let status = manager.status("worker").unwrap();
    let stalled = status["input_stalled"].as_array().unwrap();
    assert_eq!(stalled.len(), 1, "{status}");
    assert_eq!(stalled[0]["message_id"], "next", "{status}");
}
