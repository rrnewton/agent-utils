//! #2399 agentctl-muse-delivery-unrecorded-paste: a Muse delivery stopped after its paste is
//! recorded as typed, so its paste is never refused as a draft agentctl did not type, and a
//! prompt an earlier attempt pasted without recording it is submitted where it is, never typed
//! again.
use super::herdr_compatibility::{Clock, MuseEditor};
use super::*;

const LANE: &str = "[from lane-a via agentctl · reply: agentctl send lane-a]";

fn start_muse(
    fixture: &Fixture,
    busy: bool,
    collapse_over: Option<usize>,
) -> ManagedAgents<'_, Fake> {
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
    let mut editor = MuseEditor::new("YOLO", false, collapse_over);
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

fn editor<T>(fixture: &Fixture, look: impl FnOnce(&mut MuseEditor) -> T) -> T {
    look(fixture.client.muse_editor.lock().unwrap().as_mut().unwrap())
}

/// How many times `prompt` reached the agent's transcript (Enter submitted it).
fn submitted(fixture: &Fixture, prompt: &str) -> usize {
    fixture
        .client
        .scrollback
        .lock()
        .unwrap()
        .values()
        .flatten()
        .filter(|text| *text == prompt)
        .count()
}

/// How many times `prompt` was pasted into the editor.
fn pasted(fixture: &Fixture, prompt: &str) -> usize {
    let wanted = format!("{BRACKETED_PASTE_START}{prompt}{BRACKETED_PASTE_END}");
    fixture
        .client
        .literal_pastes
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, text)| *text == wanted)
        .count()
}

fn enters(fixture: &Fixture) -> usize {
    fixture
        .client
        .keys_sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, key)| key == "Enter")
        .count()
}

/// Where the queue holds message `id`, and its record, for a failure message.
fn located(queue: &Path, id: &str) -> String {
    for phase in [
        "inbox",
        "inbox.v2",
        "inflight",
        "processed",
        "failed",
        "cancelled",
    ] {
        let path = queue.join(phase).join(format!("{id}.json"));
        if let Ok(document) = agent::read_private_json(&path) {
            return format!("{phase}/{id}.json: {document}");
        }
    }
    format!("{id}: in no queue phase")
}

/// A stop (the send's deadline passing, or SIGTERM) that lands while the pane is read for the
/// first time after the paste: from then on the runtime reads cancelled.
struct StopDuringFirstReadAfterPaste<'a> {
    clock: Clock,
    client: &'a Fake,
    reads_at_paste: std::cell::Cell<Option<usize>>,
}

impl<'a> StopDuringFirstReadAfterPaste<'a> {
    fn new(client: &'a Fake) -> Self {
        Self {
            clock: Clock::default(),
            client,
            reads_at_paste: std::cell::Cell::new(None),
        }
    }
}

impl agent::AgentRuntime for StopDuringFirstReadAfterPaste<'_> {
    fn monotonic(&self) -> Duration {
        agent::AgentRuntime::monotonic(&self.clock)
    }

    fn sleep(&self, duration: Duration) {
        agent::AgentRuntime::sleep(&self.clock, duration);
    }

    fn cancelled(&self) -> bool {
        if self.client.literal_pastes.lock().unwrap().is_empty() {
            return false;
        }
        let reads = self.client.read_sources.lock().unwrap().len();
        match self.reads_at_paste.get() {
            None => {
                self.reads_at_paste.set(Some(reads));
                false
            }
            Some(at) => reads > at,
        }
    }
}

/// The inflight record of `id` at the moment each paste lands in the editor.
fn record_at_paste(fixture: &Fixture, queue: &Path, id: &str) -> Arc<Mutex<Vec<Option<Value>>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = queue.join("inflight").join(format!("{id}.json"));
    let sink = Arc::clone(&seen);
    *fixture.client.on_paste.lock().unwrap() = Some(Box::new(move |_| {
        sink.lock()
            .unwrap()
            .push(agent::read_private_json(&record).ok());
    }));
    seen
}

/// The incident: a send stopped right after its paste left the prompt pending with no attempt
/// and the paste in the editor, and every later delivery refused that paste as "a draft that
/// agentctl did not type". Now the stopped delivery is recorded as typed, and the next delivery
/// submits the paste as agentctl's own, then delivers its prompt.
#[test]
fn a_muse_delivery_stopped_after_its_paste_is_recorded_typed_and_the_next_one_submits_it() {
    for busy in [true, false] {
        let fixture = Fixture::new();
        let manager = start_muse(&fixture, busy, None);
        let queue = manager.queue("coord").unwrap();
        let first = format!("{LANE}\nstep 2 measurements: set 1 exclusive bytes, set 2 bounds");
        let stop = StopDuringFirstReadAfterPaste::new(&fixture.client);
        let _ = manager.send_identified_with_runtime("coord", &first, quick(), "first", &stop);
        assert_eq!(
            editor(&fixture, |editor| editor.staged().map(str::to_owned)),
            Some(first.clone()),
            "busy={busy}: the stopped delivery left its paste in the editor"
        );
        let after_stop = located(&queue, "first");

        let second = format!("{LANE}\nare the step 2 numbers in?");
        let result = manager
            .send_identified_with_runtime("coord", &second, quick(), "second", &Clock::default())
            .unwrap_or_else(|error| {
                panic!(
                    "busy={busy}: the next delivery refused: {error}\n\
                     the stopped prompt's record after the stop: {after_stop}"
                )
            });
        assert_eq!(result.delivered, ["second"], "busy={busy}");
        let failed = agent::read_private_json(&queue.join("failed/first.json"))
            .unwrap_or_else(|_| panic!("busy={busy}: {after_stop}"));
        assert_eq!(failed["possibly_submitted"], true, "busy={busy}: {failed}");
        assert_eq!(failed["delivery_attempts"], 1, "busy={busy}: {failed}");
        assert!(
            failed.get("draft_recovered_at").is_some(),
            "busy={busy}: the next delivery submitted the paste as agentctl's: {failed}"
        );
        assert_eq!(submitted(&fixture, &first), 1, "busy={busy}");
        assert_eq!(submitted(&fixture, &second), 1, "busy={busy}");
        assert_eq!(
            pasted(&fixture, &first),
            1,
            "busy={busy}: never typed again"
        );
        assert_eq!(pasted(&fixture, &second), 1, "busy={busy}");
        assert!(
            editor(&fixture, |editor| editor.staged().is_none()),
            "busy={busy}"
        );
    }
}

#[test]
fn the_typing_stamp_is_on_the_queue_record_before_a_muse_paste_lands() {
    for busy in [true, false] {
        let fixture = Fixture::new();
        let manager = start_muse(&fixture, busy, None);
        let queue = manager.queue("coord").unwrap();
        let seen = record_at_paste(&fixture, &queue, "stamp");
        let prompt = format!("{LANE}\nstamp me before you type me");
        let result = manager
            .send_identified_with_runtime("coord", &prompt, quick(), "stamp", &Clock::default())
            .unwrap();
        assert_eq!(result.delivered, ["stamp"], "busy={busy}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "busy={busy}: one paste");
        let record = seen[0]
            .as_ref()
            .unwrap_or_else(|| panic!("busy={busy}: the prompt was inflight while it was typed"));
        assert!(
            record["paste_started_at"].as_f64().is_some(),
            "busy={busy}: {record}"
        );
    }
}

/// A drainer that dies right after its paste leaves the inflight record as it was at the paste.
/// The next drain recovers it as stranded and flushes the paste with Enter: never as a prompt
/// that was never typed, which it would type again over its own paste, or refuse.
#[test]
fn a_drainer_that_dies_after_a_muse_paste_is_recovered_stranded_and_flushed_not_retyped() {
    for busy in [true, false] {
        let prompt = format!("{LANE}\nthe drain that typed me died before Enter");
        // The record a real drain leaves at the moment of its paste.
        let snapshot = {
            let fixture = Fixture::new();
            let manager = start_muse(&fixture, busy, None);
            let queue = manager.queue("coord").unwrap();
            let seen = record_at_paste(&fixture, &queue, "crash");
            manager
                .send_identified_with_runtime("coord", &prompt, quick(), "crash", &Clock::default())
                .unwrap();
            let snapshot = seen.lock().unwrap()[0]
                .clone()
                .expect("inflight at the paste");
            snapshot
        };

        let fixture = Fixture::new();
        let manager = start_muse(&fixture, busy, None);
        let queue = manager.queue("coord").unwrap();
        manager
            .queue_identified("coord", &prompt, None, None, Some("crash"))
            .unwrap();
        // The drainer died after the paste: its claim is in inflight/, its paste in the editor.
        fs::remove_file(queue.join("inbox/crash.json")).unwrap();
        agent::atomic_json(&queue.join("inflight/crash.json"), &snapshot).unwrap();
        editor(&fixture, |editor| editor.draft(&prompt));

        let next = format!("{LANE}\nanything else?");
        let result = manager
            .send_identified_with_runtime("coord", &next, quick(), "next", &Clock::default())
            .unwrap_or_else(|error| {
                panic!("busy={busy}: {error}\nrecord at the paste: {snapshot}")
            });
        assert!(
            result.delivered.contains(&"crash".to_owned()),
            "busy={busy}: {result:?}"
        );
        assert!(
            result.delivered.contains(&"next".to_owned()),
            "busy={busy}: {result:?}"
        );
        let processed = agent::read_private_json(&queue.join("processed/crash.json")).unwrap();
        assert!(
            processed["stranded_at"].as_f64().is_some(),
            "busy={busy}: recovered as stranded: {processed}"
        );
        assert_eq!(
            pasted(&fixture, &prompt),
            0,
            "busy={busy}: never typed again"
        );
        assert_eq!(submitted(&fixture, &prompt), 1, "busy={busy}");
        assert_eq!(submitted(&fixture, &next), 1, "busy={busy}");
    }
}

/// What older drainers leave behind: the prompt pending with no attempt recorded and its own
/// paste in the editor. The delivery submits that copy as the prompt, by its text or by a
/// placeholder of its unique size, and never pastes it a second time.
#[test]
fn a_pending_prompt_whose_own_paste_is_in_the_editor_is_submitted_there_not_typed_again() {
    for busy in [true, false] {
        for collapse_over in [None, Some(40)] {
            let label = format!("busy={busy} collapse_over={collapse_over:?}");
            let fixture = Fixture::new();
            let manager = start_muse(&fixture, busy, collapse_over);
            let queue = manager.queue("coord").unwrap();
            let own = format!("{LANE}\nleft in the editor by an older drainer, never recorded");
            manager
                .queue_identified("coord", &own, None, None, Some("own"))
                .unwrap();
            editor(&fixture, |editor| editor.draft(&own));
            if collapse_over.is_some() {
                let screen = editor(&fixture, |editor| editor.screen());
                assert!(screen.contains("[Pasted Content "), "{label}: {screen}");
            }

            let next = format!("{LANE}\nand the next message after it, also long enough");
            let result = manager
                .send_identified_with_runtime("coord", &next, quick(), "next", &Clock::default())
                .unwrap_or_else(|error| panic!("{label}: {error}\n{}", located(&queue, "own")));
            assert_eq!(result.delivered, ["own", "next"], "{label}");
            assert_eq!(
                pasted(&fixture, &own),
                0,
                "{label}: never pasted a second time"
            );
            assert_eq!(pasted(&fixture, &next), 1, "{label}");
            assert_eq!(submitted(&fixture, &own), 1, "{label}");
            assert_eq!(submitted(&fixture, &next), 1, "{label}");
            let processed = agent::read_private_json(&queue.join("processed/own.json")).unwrap();
            assert!(
                matches!(
                    processed["delivery_state"].as_str(),
                    Some("processed" | "queued")
                ),
                "{label}: {processed}"
            );
            assert!(
                editor(&fixture, |editor| editor.staged().is_none()),
                "{label}"
            );
        }
    }
}

/// The recognition stays narrow: a draft that only may be the prompt (a placeholder whose size a
/// prompt agentctl recorded typing shares), an unframed prompt's text, and anyone else's text
/// are left exactly as they are, nothing is pressed or typed, and the prompt stays pending.
#[test]
fn a_draft_that_only_may_be_the_prompt_or_is_not_agentctls_is_left_alone() {
    for busy in [true, false] {
        // A placeholder of a size an earlier delivery shares.
        let fixture = Fixture::new();
        let manager = start_muse(&fixture, busy, Some(40));
        let queue = manager.queue("coord").unwrap();
        let earlier = format!("{LANE}\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let pending = format!("{LANE}\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(earlier.chars().count(), pending.chars().count());
        manager
            .send_identified_with_runtime("coord", &earlier, quick(), "earlier", &Clock::default())
            .unwrap();
        manager
            .queue_identified("coord", &pending, None, None, Some("pending"))
            .unwrap();
        editor(&fixture, |editor| editor.draft(&pending));
        let keys_before = enters(&fixture);
        let error = manager
            .send_identified_with_runtime(
                "coord",
                &format!("{LANE}\nnext"),
                quick(),
                "next",
                &Clock::default(),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("it may be either"),
            "busy={busy}: {error}"
        );
        assert_eq!(
            enters(&fixture),
            keys_before,
            "busy={busy}: nothing pressed"
        );
        assert_eq!(pasted(&fixture, &pending), 0, "busy={busy}");
        assert_eq!(
            editor(&fixture, |editor| editor.staged().map(str::to_owned)),
            Some(pending.clone()),
            "busy={busy}"
        );
        assert!(
            queue.join("inbox/pending.json").exists(),
            "busy={busy}: {}",
            located(&queue, "pending")
        );

        // An unframed prompt's text, and someone else's text.
        for (queued, draft) in [
            (
                "deploy it now please, the gate is green".to_owned(),
                "deploy it now please, the gate is green".to_owned(),
            ),
            (
                format!("{LANE}\nthe queued prompt"),
                "a person is typing this".to_owned(),
            ),
        ] {
            let fixture = Fixture::new();
            let manager = start_muse(&fixture, busy, None);
            let queue = manager.queue("coord").unwrap();
            manager
                .queue_identified("coord", &queued, None, None, Some("held"))
                .unwrap();
            editor(&fixture, |editor| editor.draft(&draft));
            let error = manager
                .send_identified_with_runtime(
                    "coord",
                    &format!("{LANE}\nnext"),
                    quick(),
                    "next",
                    &Clock::default(),
                )
                .unwrap_err();
            // A busy delivery names the draft; an idle pane whose draft no one claims reads
            // `staged`, and a zero-timeout readiness probe keeps the probe's wording.
            let refused = if busy {
                "did not type"
            } else {
                "last status=staged"
            };
            assert!(error.to_string().contains(refused), "busy={busy}: {error}");
            assert_eq!(enters(&fixture), 0, "busy={busy}: nothing pressed");
            assert!(fixture.client.literal_pastes.lock().unwrap().is_empty());
            assert_eq!(
                editor(&fixture, |editor| editor.staged().map(str::to_owned)),
                Some(draft),
                "busy={busy}"
            );
            assert!(
                queue.join("inbox/held.json").exists(),
                "busy={busy}: {}",
                located(&queue, "held")
            );
        }
    }
}
