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
