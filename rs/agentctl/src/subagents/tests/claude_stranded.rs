//! #1099 claude-stranded-false-lost: a prompt a drain pasted into a Claude Code pane and left
//! stranded can be taken without ever showing on the screen (queued, then absorbed into the
//! running turn). The pinned session's transcript is the proof; nothing else's is.
use super::herdr_compatibility::Clock;
use super::*;

const OTHER_SESSION: &str = "99999999-8888-7777-6666-555555555555";
const PROMPT: &str = "[from coordinator via agentctl · reply: agentctl send coordinator]\n\
                      Queued for after the current task: plan and ETA first.";

/// A started Claude Code agent, whose composer strands every paste, with a private home for its
/// transcripts and the conversation id it is pinned to.
fn stranding_claude(fixture: &Fixture) -> (ManagedAgents<'_, Fake>, PathBuf, String) {
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "claude".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = manager.load("worker").unwrap();
    let session = record
        .effective_native_session()
        .map(|native| native.value)
        .or_else(|| record.session_value.clone())
        .expect("a started Claude Code agent is pinned to its conversation");
    assert_ne!(session, OTHER_SESSION);
    *fixture.client.stranded_refusal.lock().unwrap() = Some(format!(
        "{}: late_paste: pane owned: pasted prompt was not observed in the claude composer \
         within 10.0s; no submission key was sent",
        crate::unknown_harness::STRANDED_PROMPT_KIND
    ));
    let home = fixture.root.join("claude-home");
    TEST_HARNESS_HOME.with(|slot| *slot.borrow_mut() = Some(home.clone()));
    (manager, home, session)
}

/// Write `entries` as the transcript of `session` in the project directory of `cwd`.
fn transcript(home: &Path, cwd: &Path, session: &str, entries: &[Value]) {
    let project: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let directory = home.join(".claude/projects").join(project);
    fs::create_dir_all(&directory).unwrap();
    let lines: Vec<String> = entries.iter().map(Value::to_string).collect();
    fs::write(
        directory.join(format!("{session}.jsonl")),
        lines.join("\n") + "\n",
    )
    .unwrap();
}

fn options() -> DrainOptions {
    DrainOptions {
        ready_timeout: Duration::ZERO,
        working_timeout: Duration::from_millis(50),
        ..DrainOptions::default()
    }
}

/// The prompt as Claude Code recorded taking it mid-turn, at a time after the strand.
fn taken() -> Vec<Value> {
    vec![
        json!({"type": "queue-operation", "operation": "enqueue",
               "timestamp": "2099-01-01T00:00:00.000Z", "content": PROMPT}),
        json!({"type": "queue-operation", "operation": "remove", "reason": "absorbed_mid_turn",
               "timestamp": "2099-01-01T00:00:01.000Z", "content": PROMPT}),
    ]
}

#[test]
fn a_stranded_prompt_the_pinned_transcript_shows_taken_is_delivered_not_lost() {
    let fixture = Fixture::new();
    let (manager, home, session) = stranding_claude(&fixture);
    let error = manager
        .send_identified_with_runtime("worker", PROMPT, options(), "brief", &Clock::default())
        .unwrap_err();
    assert_eq!(error.exit_code(), 75, "{error}");
    let queue = manager.queue("worker").unwrap();
    assert!(agent::read_private_json(&queue.join("inbox/brief.json"))
        .unwrap()
        .get("stranded_at")
        .is_some());
    let typed = fixture.client.runs.lock().unwrap().len();
    // Claude Code queued the paste and absorbed it into its running turn; the screen never
    // showed it as a user turn.
    transcript(&home, &fixture.root, &session, &taken());
    *fixture.client.stranded_refusal.lock().unwrap() = None;
    manager
        .send_identified_with_runtime("worker", "next", options(), "next", &Clock::default())
        .unwrap();
    let processed = agent::read_private_json(&queue.join("processed/brief.json")).unwrap();
    // Delivered, recorded as queued behind the turn that absorbed it.
    assert_eq!(processed["delivery_state"], "queued", "{processed}");
    assert!(!queue.join("failed/brief.json").exists());
    // Only the new prompt was typed: the stranded one never again.
    let runs = fixture.client.runs.lock().unwrap();
    assert_eq!(runs.len(), typed + 1, "{runs:?}");
    assert!(runs.last().unwrap().contains("next"), "{runs:?}");
    TEST_HARNESS_HOME.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn only_this_agents_pinned_session_and_entries_after_the_strand_count() {
    let fixture = Fixture::new();
    let (manager, home, session) = stranding_claude(&fixture);
    manager
        .send_identified_with_runtime("worker", PROMPT, options(), "brief", &Clock::default())
        .unwrap_err();
    // Another agent's session in the same project directory took the same text, and this
    // session holds it only from before the strand: neither is this prompt being taken.
    transcript(&home, &fixture.root, OTHER_SESSION, &taken());
    transcript(
        &home,
        &fixture.root,
        &session,
        &[
            json!({"type": "user", "timestamp": "2000-01-01T00:00:00.000Z",
                 "message": {"role": "user", "content": PROMPT}}),
        ],
    );
    manager
        .send_identified_with_runtime("worker", "next", options(), "next", &Clock::default())
        .unwrap_err();
    let queue = manager.queue("worker").unwrap();
    assert!(!queue.join("processed/brief.json").exists());
    let pending = agent::read_private_json(&queue.join("inbox/brief.json")).unwrap();
    assert!(pending.get("stranded_at").is_some(), "{pending}");
    TEST_HARNESS_HOME.with(|slot| *slot.borrow_mut() = None);
}
