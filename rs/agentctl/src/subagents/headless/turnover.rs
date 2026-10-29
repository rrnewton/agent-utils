//! `agentctl turnover NAME` for headless agents (#686 agent-turnover): ask the agent for one
//! self-contained handoff, then continue the same agent in a fresh harness session that reads
//! that handoff first.
//!
//! The record, its token, its queue and its name stay; only the session changes. Messages sent
//! meanwhile are never refused: they wait in the queue and reach the new session after the
//! handoff. The journal `REGISTRY/.lineage/<name>/turnover.json` names the phase, and rerunning
//! the command continues from it:
//!
//! - `asking`: the handoff request is queued; the command waits until the handoff file ends with
//!   the request's nonce line and the agent is idle. Without an answer in time it stops and
//!   changes nothing (exit 75); rerun to wait again, `--no-ask` to write a mechanical handoff,
//!   or `--abort`.
//! - `captured`: the handoff is recorded with its digest and a new session ID is chosen.
//! - `switching`: the takeover mark (`headless.takeover`, state `turnover`) goes in first, so no
//!   supervisor can start a wrapper on the old session, even one of an older build; the wrapper
//!   stops at a turn boundary; once no process writes the old session, the record switches to
//!   the new one and the mark is removed. The lineage gets `ended` (turned-over) and a
//!   `generation` with relation `turnover` and the handoff.
//! - `feeding`: the handoff prompt is queued as urgent, so it is the new session's first input
//!   (an urgent item queued earlier still goes before it).
//! - `starting`: the wrapper starts again (its systemd unit when it had one), and the journal is
//!   removed.
//!
//! `--abort` is allowed until the wrapper is asked to stop; after that, recovery only goes
//! forward.

use serde_json::Map;
use sha2::{Digest, Sha256};

use super::takeover::{now_seconds, set_marker, systemd_unit, TAKEOVER_SCHEMA};
use super::*;

/// Schema of the turnover journal.
const JOURNAL_SCHEMA: &str = "agentctl-turnover/v1";
/// The takeover mark's `state` during a turnover.
const MARK_STATE: &str = "turnover";
/// Longest handoff passed inline; a longer one is passed by its path.
const INLINE_HANDOFF_BYTES: usize = 48 * 1024;
/// How many of the last assistant messages a mechanical handoff quotes.
const MECHANICAL_MESSAGES: usize = 5;

/// Options of `agentctl turnover`.
#[derive(Clone, Debug)]
pub struct TurnoverOptions {
    /// How long to wait for the agent's handoff.
    pub handoff_timeout: Duration,
    /// How long to wait for a running turn to end before the wrapper stops.
    pub wait: Duration,
    /// Deliver the handoff request mid-turn, and interrupt a running turn before the stop.
    pub interrupt: bool,
    /// Use this file as the handoff instead of asking the agent.
    pub handoff_file: Option<PathBuf>,
    /// Do not ask: write a mechanical handoff from the agent's records.
    pub no_ask: bool,
    /// Abandon a turnover that has not stopped the wrapper yet.
    pub abort: bool,
}

fn header() -> String {
    crate::sender_header::render(
        &crate::sender_header::Sender::Other("agentctl-turnover".to_owned()),
        crate::sender_header::ROUTE,
    )
}

fn nonce_line(nonce: &str) -> String {
    format!("<!-- agentctl-turnover {nonce} -->")
}

/// The request that asks the agent for its handoff (the owner's template, 2026-10-29).
fn handoff_request(name: &str, path: &Path, nonce: &str) -> String {
    format!(
        "{}\nThis session of {name} is being replaced by a fresh session that continues your work under the same name. Write ONE self-contained handoff for your successor to this file:\n\n{}\n\nYour successor sees only this handoff and the messages still waiting for you. It does not see your original brief or this conversation, so spell everything out rather than referring to it. Cover:\n\n1. Purpose: what this lane is for now, as it stands today. It may have changed since your brief.\n2. State of the work: what is done (commits, branches, landed work, with their identifiers), what is in progress and where, what comes next, and open questions or blockers.\n3. Standing rules: every rule you work under, written out in full: the slot protocol, the gate and landing rules, how and to whom you report, and every constraint or decision you were given along the way.\n4. Open requests: anything you still owe someone (a reply, a report), and how to deliver it.\n\nWrite the whole file, and make its last line exactly:\n\n{}\n\nThen end your turn without starting other work.",
        header(),
        path.display(),
        nonce_line(nonce)
    )
}

/// The new session's first prompt: the handoff, or its path when it is long.
fn handoff_prompt(name: &str, old_session: &str, handoff: &str, path: &Path) -> String {
    let body = if handoff.len() <= INLINE_HANDOFF_BYTES {
        handoff.trim_end().to_owned()
    } else {
        format!(
            "The handoff is {} bytes; read it in full from {} before doing anything else.",
            handoff.len(),
            path.display()
        )
    };
    format!(
        "{}\nYou are {name}. You continue the work of this agent's previous session ({old_session}), which was turned over to you. Its handoff follows, and it is all you have of that session. Messages that were still waiting for it come after this one.\n\n{body}",
        header()
    )
}

/// The handoff without its closing nonce line.
fn without_nonce(text: &str, nonce: &str) -> String {
    let line = nonce_line(nonce);
    text.lines()
        .filter(|candidate| candidate.trim() != line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_journal(path: &Path) -> Result<Option<Map<String, Value>>> {
    match fs::read(path) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|error| fail(format!("unreadable {}: {error}", path.display())))?;
            match value {
                Value::Object(journal) if journal.get("schema") == Some(&json!(JOURNAL_SCHEMA)) => {
                    Ok(Some(journal))
                }
                _ => Err(fail(format!(
                    "{} is not a turnover journal this build reads",
                    path.display()
                ))),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fail(format!("read {}: {error}", path.display()))),
    }
}

fn write_journal(path: &Path, journal: &Map<String, Value>) -> Result<()> {
    agent::atomic_json(path, &Value::Object(journal.clone()))
        .map_err(|error| fail(format!("write {}: {error}", path.display())))
}

fn field<'a>(journal: &'a Map<String, Value>, key: &str) -> &'a str {
    journal.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The next handoff number: one more than the largest `<n>.md` in `handoffs`.
fn next_handoff(handoffs: &Path) -> u64 {
    fs::read_dir(handoffs)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    entry
                        .file_name()
                        .to_str()?
                        .strip_suffix(".md")?
                        .parse::<u64>()
                        .ok()
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
        + 1
}

fn write_new_file(path: &Path, text: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| fail(format!("create {}: {error}", path.display())))?;
    file.write_all(text.as_bytes())
        .map_err(|error| fail(format!("write {}: {error}", path.display())))
}

fn git_summary(cwd: &str) -> String {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .trim_end()
                    .to_owned()
            })
    };
    match (
        run(&["status", "--short", "--branch"]),
        run(&["log", "--oneline", "-5"]),
    ) {
        (Some(status), Some(log)) => {
            format!("```text\n$ git status --short --branch\n{status}\n$ git log --oneline -5\n{log}\n```")
        }
        _ => "(not a Git checkout, or git failed)".to_owned(),
    }
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    /// A handoff assembled from the agent's records, for `--no-ask`.
    fn mechanical_handoff(&self, record: &AgentRecord, old_session: &str) -> Result<String> {
        let directory = self.directory(&record.name)?;
        let events = events::read_recent(&events::directory(&directory), 4 * 1024 * 1024)
            .unwrap_or_default();
        let messages: Vec<String> = events
            .iter()
            .filter(|event| event.kind == "assistant.text")
            .filter_map(|event| event.data["text"].as_str())
            .rev()
            .take(MECHANICAL_MESSAGES)
            .map(|text| events::excerpt(text, 2000))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let pending = agent_queue::queue::pending_identifiers(&self.queue(&record.name)?)
            .map(|ids| ids.len())
            .unwrap_or(0);
        let mut text = format!(
            "# Mechanical handoff for {}\n\nThe previous session ({old_session}) was not asked for a handoff (`agentctl turnover --no-ask`). agentctl assembled this from its records, so it is incomplete: read the working tree and the agent's transcript before relying on it.\n\n- Working directory: {}\n- Inputs still queued: {pending}\n\n## Last messages of the previous session\n",
            record.name, record.cwd
        );
        if messages.is_empty() {
            text.push_str("\n(none in the event log)\n");
        }
        for message in messages {
            text.push_str(&format!("\n> {}\n", message.replace('\n', "\n> ")));
        }
        text.push_str(&format!(
            "\n## Working tree\n\n{}\n",
            git_summary(&record.cwd)
        ));
        Ok(text)
    }

    /// Turn a headless agent over to a fresh session: see the module documentation.
    pub fn turnover_headless(&self, agent_name: &str, options: &TurnoverOptions) -> Result<Value> {
        let record = self.load(agent_name)?;
        if !record.is_headless() {
            return Err(fail(format!(
                "turnover in this build serves headless agents; {agent_name:?} is {} ({})",
                record.mode, record.adapter
            )));
        }
        let journal_path = crate::lineage::turnover_path(&self.registry, agent_name);
        let journal = read_journal(&journal_path)?;
        if options.abort {
            return match journal {
                None => Err(fail(format!("no turnover of {agent_name:?} is in progress"))),
                Some(journal) if matches!(field(&journal, "phase"), "asking" | "captured") => {
                    fs::remove_file(&journal_path)
                        .map_err(|error| fail(format!("remove {}: {error}", journal_path.display())))?;
                    Ok(json!({"name": agent_name, "aborted": true,
                              "phase": journal["phase"], "handoff": journal["handoff_path"]}))
                }
                Some(journal) => Err(fail(format!(
                    "the turnover of {agent_name:?} is past its point of no return ({}); rerun agentctl turnover {agent_name} to finish it",
                    field(&journal, "phase")
                ))),
            };
        }
        if record.lifecycle != "running" {
            return Err(fail(format!(
                "agent {agent_name:?} is {}; nothing to turn over",
                record.lifecycle
            )));
        }
        let mut journal = match journal {
            Some(journal) => {
                if field(&journal, "token") != record.token {
                    return Err(fail(format!(
                        "agent {agent_name:?} was replaced since its turnover began; agentctl turnover {agent_name} --abort, or remove {}",
                        journal_path.display()
                    )));
                }
                journal
            }
            None => self.begin_turnover(&record, options, &journal_path)?,
        };
        if field(&journal, "phase") == "asking"
            && !self.await_handoff(&record, &mut journal, options, &journal_path)?
        {
            return Ok(json!({
                "name": agent_name,
                "turned_over": false,
                "phase": "asking",
                "timed_out": true,
                "handoff": journal["handoff_path"],
                "message": format!(
                    "no complete handoff within {} s; {agent_name} is unchanged. Rerun to wait again, pass --no-ask for a mechanical handoff, or --abort",
                    options.handoff_timeout.as_secs()
                ),
            }));
        }
        if field(&journal, "phase") == "captured" {
            let path = PathBuf::from(field(&journal, "handoff_path"));
            let bytes = fs::read(&path)
                .map_err(|error| fail(format!("read the handoff {}: {error}", path.display())))?;
            journal.insert("handoff_sha256".to_owned(), json!(sha256_hex(&bytes)));
            let session = claude::random_uuid().map_err(|error| fail(error.to_string()))?;
            journal.insert("new_session".to_owned(), json!(session));
            journal.insert("phase".to_owned(), json!("switching"));
            write_journal(&journal_path, &journal)?;
        }
        if field(&journal, "phase") == "switching" {
            self.switch_session(&record, &mut journal, options, &journal_path)?;
        }
        if field(&journal, "phase") == "feeding" {
            let path = PathBuf::from(field(&journal, "handoff_path"));
            let text = fs::read_to_string(&path)
                .map_err(|error| fail(format!("read the handoff {}: {error}", path.display())))?;
            let prompt = handoff_prompt(
                agent_name,
                field(&journal, "old_session"),
                &without_nonce(&text, field(&journal, "nonce")),
                &path,
            );
            let queue_root = self.queue(agent_name)?;
            // A fixed ID, so a rerun after a crash right after queueing does not queue it twice.
            let id = format!("turnover-{}", field(&journal, "nonce"));
            if agent_queue::queue::message_state(&queue_root, &id)?.is_none() {
                self.enqueue_headless(&queue_root, &prompt, Some(&id), "urgent", "now", None)?;
            }
            journal.insert("prompt_id".to_owned(), json!(id));
            journal.insert("phase".to_owned(), json!("starting"));
            write_journal(&journal_path, &journal)?;
        }
        // `starting`: bring the wrapper back on the new session.
        let socket = hcontrol::socket_path(&self.directory(agent_name)?);
        let wrapper =
            if hcontrol::request(&socket, &json!({"op": "status"}), CONTROL_TIMEOUT).is_ok() {
                json!({"already_running": true})
            } else {
                self.start_wrapper_again(
                    agent_name,
                    journal.get("systemd_unit").and_then(Value::as_str),
                )?
            };
        fs::remove_file(&journal_path)
            .map_err(|error| fail(format!("remove {}: {error}", journal_path.display())))?;
        Ok(json!({
            "name": agent_name,
            "turned_over": true,
            "old_session": journal["old_session"],
            "new_session": journal["new_session"],
            "handoff": journal["handoff_path"],
            "handoff_sha256": journal["handoff_sha256"],
            "handoff_kind": journal["handoff_kind"],
            "prompt_id": journal["prompt_id"],
            "wrapper": wrapper,
            "next": format!("agentctl tail {agent_name} --follow"),
        }))
    }

    /// Write the journal of a new turnover, with its handoff captured already when the operator
    /// supplied one or asked for a mechanical one, and otherwise queue the handoff request.
    fn begin_turnover(
        &self,
        record: &AgentRecord,
        options: &TurnoverOptions,
        journal_path: &Path,
    ) -> Result<Map<String, Value>> {
        if takeover_marker(record).is_some() {
            return Err(fail(format!(
                "agent {:?} is taken over or turning over; finish that first (agentctl handback {0}, or agentctl turnover {0})",
                record.name
            )));
        }
        if record.paused {
            return Err(fail(format!("agent {:?} is paused", record.name)));
        }
        let old_session = record.headless_session()?;
        let handoffs =
            crate::lineage::handoffs_directory(&self.registry, &record.name).map_err(fail)?;
        let path = handoffs.join(format!("{}.md", next_handoff(&handoffs)));
        let nonce = claude::random_uuid().map_err(|error| fail(error.to_string()))?;
        let mut journal = Map::new();
        for (key, value) in [
            ("schema", json!(JOURNAL_SCHEMA)),
            ("name", json!(record.name)),
            ("token", json!(record.token)),
            ("old_session", json!(old_session)),
            ("nonce", json!(nonce)),
            ("handoff_path", json!(path.display().to_string())),
            ("started_at", json!(now_seconds())),
        ] {
            journal.insert(key.to_owned(), value);
        }
        if let Some(source) = &options.handoff_file {
            let text = fs::read_to_string(source)
                .map_err(|error| fail(format!("read {}: {error}", source.display())))?;
            write_new_file(&path, &text)?;
            journal.insert("handoff_kind".to_owned(), json!("file"));
            journal.insert("phase".to_owned(), json!("captured"));
        } else if options.no_ask {
            write_new_file(&path, &self.mechanical_handoff(record, &old_session)?)?;
            journal.insert("handoff_kind".to_owned(), json!("mechanical"));
            journal.insert("phase".to_owned(), json!("captured"));
        } else {
            journal.insert("handoff_kind".to_owned(), json!("agent"));
            journal.insert("phase".to_owned(), json!("asking"));
        }
        // The journal first, so a rerun after a crash finds the request it made.
        write_journal(journal_path, &journal)?;
        if field(&journal, "phase") == "asking" {
            let queued = self.send_headless(
                &record.name,
                &handoff_request(&record.name, &path, &nonce),
                None,
                options.interrupt,
                None,
                Duration::ZERO,
            )?;
            journal.insert("request_id".to_owned(), json!(queued.message_id));
            write_journal(journal_path, &journal)?;
        }
        Ok(journal)
    }

    /// Wait until the handoff file ends with the nonce line and the agent is idle. `false` when
    /// the timeout passed first; the journal then says so.
    fn await_handoff(
        &self,
        record: &AgentRecord,
        journal: &mut Map<String, Value>,
        options: &TurnoverOptions,
        journal_path: &Path,
    ) -> Result<bool> {
        let path = PathBuf::from(field(journal, "handoff_path"));
        let line = nonce_line(field(journal, "nonce"));
        let socket = hcontrol::socket_path(&self.directory(&record.name)?);
        let deadline = Instant::now() + options.handoff_timeout;
        loop {
            let complete = fs::read_to_string(&path).is_ok_and(|text| {
                text.lines()
                    .rev()
                    .find(|row| !row.trim().is_empty())
                    .is_some_and(|last| last.trim() == line)
            });
            if complete {
                match hcontrol::request(&socket, &json!({"op": "status"}), CONTROL_TIMEOUT) {
                    Ok(status) if status["busy"] != true && status["current"].is_null() => {
                        journal.insert("phase".to_owned(), json!("captured"));
                        journal.remove("timed_out");
                        write_journal(journal_path, journal)?;
                        return Ok(true);
                    }
                    Ok(_) => {}
                    Err(hcontrol::ClientError::NotRunning(_)) => {
                        // The handoff is whole and nothing runs the session: as idle as it gets.
                        journal.insert("phase".to_owned(), json!("captured"));
                        write_journal(journal_path, journal)?;
                        return Ok(true);
                    }
                    Err(_) => {}
                }
            }
            if Instant::now() >= deadline {
                journal.insert("timed_out".to_owned(), json!(true));
                write_journal(journal_path, journal)?;
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Fence the old session with the takeover mark, stop the wrapper, switch the record to the
    /// new session, and record the lineage.
    fn switch_session(
        &self,
        record: &AgentRecord,
        journal: &mut Map<String, Value>,
        options: &TurnoverOptions,
        journal_path: &Path,
    ) -> Result<()> {
        let name = record.name.clone();
        let old_session = field(journal, "old_session").to_owned();
        let new_session = field(journal, "new_session").to_owned();
        let mark_token = field(journal, "nonce").to_owned();
        let current = self.load(&name)?;
        let switched = current.headless_session().ok().as_deref() == Some(new_session.as_str());
        if !switched {
            let directory = self.directory(&name)?;
            let ours =
                takeover_marker(&current).is_some_and(|marker| marker["token"] == mark_token);
            if !ours {
                if takeover_marker(&current).is_some() {
                    return Err(fail(format!(
                        "agent {name:?} was taken over meanwhile; finish that first"
                    )));
                }
                let socket = hcontrol::socket_path(&directory);
                let unit = hcontrol::request(&socket, &json!({"op": "status"}), CONTROL_TIMEOUT)
                    .ok()
                    .and_then(|status| status["wrapper_pid"].as_u64())
                    .and_then(systemd_unit);
                journal.insert("systemd_unit".to_owned(), json!(unit));
                write_journal(journal_path, journal)?;
                let _lock = self.lock(&name)?;
                let mut fenced = self.load(&name)?;
                if fenced.token != record.token || takeover_marker(&fenced).is_some() {
                    return Err(fail(format!("agent {name:?} changed meanwhile")));
                }
                set_marker(
                    &mut fenced,
                    Some(json!({
                        "schema": TAKEOVER_SCHEMA,
                        "token": mark_token,
                        "state": MARK_STATE,
                        "purpose": "turnover",
                        "since": now_seconds(),
                        "systemd_unit": unit,
                    })),
                );
                self.save(&fenced)?;
            }
            let takeover = TakeoverOptions {
                workspace: None,
                interrupt: options.interrupt,
                wait: options.wait,
                claude_bin: None,
            };
            if let Err(failure) = self.stop_wrapper_for_takeover(&directory, &takeover) {
                if !failure.stop_requested && !ours {
                    // The wrapper still serves the old session: lift the fence, change nothing.
                    let _ = self.change_marker(&name, &record.token, &mark_token, |_| false);
                    return Err(failure.error);
                }
                return Err(fail(format!(
                    "{}; the turnover of {name} keeps its place: rerun agentctl turnover {name} once the wrapper has exited",
                    failure.error
                )));
            }
            let writers = wrapper::session_processes(&old_session);
            if !writers.is_empty() {
                return Err(fail(format!(
                    "process(es) {writers:?} still run Claude Code on session {old_session}; rerun agentctl turnover {name} once they exit"
                )));
            }
            let _lock = self.lock(&name)?;
            let mut switching = self.load(&name)?;
            if switching.token != record.token {
                return Err(fail(format!("agent {name:?} was replaced meanwhile")));
            }
            if let Some(spec) = switching
                .extra
                .get_mut("headless")
                .and_then(Value::as_object_mut)
            {
                spec.insert("session".to_owned(), json!(new_session));
            }
            set_marker(&mut switching, None);
            self.save(&switching)?;
        }
        // Repeats are recognised by the lineage, so a rerun after a crash writes these once.
        let mut ended = Map::new();
        for (key, value) in [
            ("kind", json!("ended")),
            ("reason", json!("turned-over")),
            ("token", json!(record.token)),
            ("harness", json!(record.harness)),
            ("session", json!(old_session)),
            ("cwd", json!(record.cwd)),
        ] {
            ended.insert(key.to_owned(), value);
        }
        let mut generation = Map::new();
        for (key, value) in [
            ("kind", json!("generation")),
            ("relation", json!("turnover")),
            ("token", json!(record.token)),
            ("harness", json!(record.harness)),
            ("session", json!(new_session)),
            ("cwd", json!(record.cwd)),
            (
                "predecessor",
                json!({"token": record.token, "session": old_session}),
            ),
            (
                "handoff",
                json!({"path": journal["handoff_path"], "sha256": journal["handoff_sha256"],
                       "kind": journal["handoff_kind"]}),
            ),
        ] {
            generation.insert(key.to_owned(), value);
        }
        if let Some(model) = &record.model {
            generation.insert("model".to_owned(), json!(model));
        }
        for entry in [ended, generation] {
            if let Err(error) = crate::lineage::append(&self.registry, &name, entry) {
                eprintln!("agentctl: lineage of {name} not recorded: {error}");
            }
        }
        journal.insert("phase".to_owned(), json!("feeding"));
        write_journal(journal_path, journal)
    }

    /// Start the wrapper again: its systemd unit when it ran in one, otherwise detached.
    fn start_wrapper_again(&self, agent_name: &str, unit: Option<&str>) -> Result<Value> {
        let mut unit_error = None;
        if let Some(unit) = unit {
            match std::process::Command::new("systemctl")
                .args(["--user", "start", unit])
                .output()
            {
                Ok(output) if output.status.success() => return Ok(json!({"systemd_unit": unit})),
                Ok(output) => {
                    unit_error = Some(String::from_utf8_lossy(&output.stderr).trim().to_owned());
                }
                Err(error) => unit_error = Some(error.to_string()),
            }
        }
        let pid = self.launch_wrapper(agent_name)?;
        Ok(
            json!({"pid": pid, "systemd_unit_failed": unit.map(|unit| json!({"unit": unit, "error": unit_error}))}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_names_the_file_the_nonce_and_the_owners_three_parts() {
        let text = handoff_request(
            "cl-foo",
            Path::new("/r/.lineage/cl-foo/handoffs/1.md"),
            "n-1",
        );
        assert!(
            text.starts_with("[from agentctl-turnover via agentctl]\n"),
            "{text}"
        );
        assert!(text.contains("/r/.lineage/cl-foo/handoffs/1.md"));
        assert!(text.contains("<!-- agentctl-turnover n-1 -->"));
        for part in [
            "Purpose",
            "State of the work",
            "Standing rules",
            "original brief",
        ] {
            assert!(text.contains(part), "{part}: {text}");
        }
        let prompt = handoff_prompt("cl-foo", "old-1", "the handoff", Path::new("/h.md"));
        assert!(prompt.contains("previous session (old-1)") && prompt.ends_with("the handoff"));
        let long = "x".repeat(INLINE_HANDOFF_BYTES + 1);
        assert!(handoff_prompt("cl-foo", "old-1", &long, Path::new("/h.md")).contains("from /h.md"));
        assert_eq!(
            without_nonce("a\nb\n<!-- agentctl-turnover n-1 -->\n", "n-1"),
            "a\nb"
        );
    }

    #[test]
    fn handoff_numbers_follow_the_largest_one() {
        let directory =
            std::env::temp_dir().join(format!("agentctl-handoffs-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        assert_eq!(next_handoff(&directory), 1);
        fs::write(directory.join("1.md"), "a").unwrap();
        fs::write(directory.join("7.md"), "b").unwrap();
        fs::write(directory.join("notes.txt"), "c").unwrap();
        assert_eq!(next_handoff(&directory), 8);
        fs::remove_dir_all(directory).unwrap();
    }
}
