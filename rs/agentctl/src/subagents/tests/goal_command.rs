//! A `/goal` command typed raw, as a chat request that is one asks for, is proved by the
//! harness's answer to it rather than by a user turn or a working state.
use super::herdr_compatibility::{Clock, MuseEditor};
use super::*;

fn quick() -> DrainOptions {
    DrainOptions {
        ready_timeout: Duration::ZERO,
        working_timeout: Duration::from_millis(50),
        ..DrainOptions::default()
    }
}

#[test]
fn a_muse_goal_command_is_pasted_whole_and_proved_by_muses_answer() {
    let fixture = Fixture::new();
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
    *fixture.client.status.lock().unwrap() = Some("done".to_owned());
    *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::new("YOLO", false, None));
    let command = "/goal ship the release and report back";
    let result = manager
        .send_identified_with_runtime("worker", command, quick(), "goal", &Clock::default())
        .unwrap();
    assert_eq!(result.delivered, ["goal"]);
    assert_eq!(
        *fixture.client.literal_pastes.lock().unwrap(),
        [(
            "owned".to_owned(),
            format!("{BRACKETED_PASTE_START}{command}{BRACKETED_PASTE_END}")
        )]
    );
    assert_eq!(
        *fixture.client.keys_sent.lock().unwrap(),
        [("owned".to_owned(), "Enter".to_owned())]
    );
    let screen = fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .screen();
    assert!(screen.contains("◆ Goal set — ship the release and report back"));
}

#[test]
fn a_codex_goal_command_is_proved_by_its_goal_active_row() {
    let fixture = Fixture::new();
    fixture.start(None);
    *fixture.client.screen.lock().unwrap() =
        Some("› Ask Codex to do anything\n  GPT · /work\n".to_owned());
    *fixture.client.screen_after_run.lock().unwrap() = Some(
        "• Goal active Objective: ship the release\n› Ask Codex to do anything\n  GPT · /work   \
         Pursuing goal (3s)\n"
            .to_owned(),
    );
    let manager = fixture.manager();
    let result = manager
        .send_identified_with_runtime(
            "worker",
            "/goal ship the release",
            quick(),
            "goal",
            &Clock::default(),
        )
        .unwrap();
    assert_eq!(result.delivered, ["goal"]);
    assert_eq!(
        fixture
            .client
            .runs
            .lock()
            .unwrap()
            .last()
            .map(String::as_str),
        Some("/goal ship the release")
    );
}

#[test]
fn a_goal_command_with_no_answer_is_possibly_submitted_and_never_typed_again() {
    let fixture = Fixture::new();
    fixture.start(None);
    *fixture.client.screen.lock().unwrap() =
        Some("› Ask Codex to do anything\n  GPT · /work\n".to_owned());
    let manager = fixture.manager();
    let runs = fixture.client.runs.lock().unwrap().len();
    let error = manager
        .send_identified_with_runtime("worker", "/goal clear", quick(), "goal", &Clock::default())
        .unwrap_err();
    assert!(error.to_string().contains("no answer"), "{error}");
    assert!(!matches!(error, AgentError::Pending(_)), "{error}");
    assert_eq!(fixture.client.runs.lock().unwrap().len(), runs + 1);
    manager
        .drain_with_runtime("worker", quick(), &Clock::default())
        .unwrap();
    assert_eq!(fixture.client.runs.lock().unwrap().len(), runs + 1);
}

/// A started Muse worker whose editor has `goal` set, idle.
fn muse_with_goal<'a>(fixture: &'a Fixture, goal: &str) -> ManagedAgents<'a, Fake> {
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
    *fixture.client.status.lock().unwrap() = Some("done".to_owned());
    let mut editor = MuseEditor::new("YOLO", false, None);
    editor.goal = Some(goal.to_owned());
    *fixture.client.muse_editor.lock().unwrap() = Some(editor);
    manager
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

fn editor<T>(fixture: &Fixture, read: impl Fn(&MuseEditor) -> T) -> T {
    read(fixture.client.muse_editor.lock().unwrap().as_ref().unwrap())
}

#[test]
fn the_goal_muse_already_has_is_kept_so_its_progress_is_not_reset() {
    let fixture = Fixture::new();
    let manager = muse_with_goal(&fixture, "ship the release");
    let result = manager
        .send_identified_with_runtime(
            "worker",
            "/goal ship the release",
            quick(),
            "goal",
            &Clock::default(),
        )
        .unwrap();
    assert_eq!(result.delivered, ["goal"]);
    // Enter submits; Up selects Keep; Enter keeps; ctrl+u takes the command's text back out.
    assert_eq!(keys(&fixture), ["Enter", "Up", "Enter", "ctrl+u"]);
    assert!(editor(&fixture, |editor| editor.question.is_none()));
    assert_eq!(
        editor(&fixture, |editor| editor.staged().map(str::to_owned)),
        None
    );
    assert!(!editor(&fixture, MuseEditor::screen).contains("Goal set"));
    // The queue records the outcome for the chat bridge's reply.
    assert_eq!(
        manager.goal_outcome("worker", "goal"),
        (Some("goal unchanged (already active)".to_owned()), None)
    );
}

#[test]
fn a_different_goal_replaces_the_current_one_and_is_proved_by_goal_set() {
    let fixture = Fixture::new();
    let manager = muse_with_goal(&fixture, "an older objective");
    let result = manager
        .send_identified_with_runtime(
            "worker",
            "/goal ship the release",
            quick(),
            "goal",
            &Clock::default(),
        )
        .unwrap();
    assert_eq!(result.delivered, ["goal"]);
    assert_eq!(keys(&fixture), ["Enter", "Enter"]);
    assert!(editor(&fixture, MuseEditor::screen).contains("◆ Goal set — ship the release"));
}

#[test]
fn a_question_that_names_another_objective_is_cancelled_never_left_open() {
    let fixture = Fixture::new();
    let manager = muse_with_goal(&fixture, "an older objective");
    fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .question_shows = Some("something else entirely".to_owned());
    let error = manager
        .send_identified_with_runtime(
            "worker",
            "/goal ship the release",
            quick(),
            "goal",
            &Clock::default(),
        )
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("cancelled with Esc and the editor cleared"),
        "{message}"
    );
    assert!(message.contains("not the one sent"), "{message}");
    assert_eq!(keys(&fixture), ["Enter", "Escape", "ctrl+u"]);
    assert!(editor(&fixture, |editor| editor.question.is_none()));
    assert_eq!(
        editor(&fixture, |editor| editor.staged().map(str::to_owned)),
        None
    );
    assert_eq!(
        editor(&fixture, |editor| editor.goal.clone()).as_deref(),
        Some("an older objective")
    );
    let (answer, failure) = manager.goal_outcome("worker", "goal");
    assert_eq!(answer, None);
    assert!(failure.is_some_and(|failure| failure.contains("cancelled with Esc")));
    // Possibly submitted as far as the queue knows, so never typed again.
    manager
        .drain_with_runtime("worker", quick(), &Clock::default())
        .unwrap();
    assert_eq!(keys(&fixture).len(), 3);
}
