//! Replayable launch specifications kept in an agent record.
//!
//! A launch spec is what ran: the harness, the program, the exact argument vector after the
//! program, the working directory, the private profile whose environment values the launch used
//! (names only are stored), the slot policy, the workspace label, and the native conversation.
//! `start` writes one before the harness exists; `adopt` and `anchor` read it from the pinned
//! harness process's `/proc/PID/cmdline`. `revive` replays it with the conversation selector
//! replaced by an explicit resume, so a profile edited after launch does not change what a
//! revived agent runs.
//!
//! The spec lives in the record under `launch_spec`. Both editions validate the same strict
//! shape and preserve the key through every lifecycle operation.

use super::*;

pub(super) const SCHEMA: &str = "agentctl-launch-spec/v1";
const SOURCES: [&str; 4] = ["start", "adopt", "anchor", "revive"];
/// Where the conversation came from: chosen by agentctl, reported by Herdr, read from the
/// harness's own argument vector, or supplied by the caller.
const NATIVE_SOURCES: [&str; 4] = ["asserted", "observed", "argv", "caller"];
const KEYS: [&str; 14] = [
    "schema",
    "source",
    "captured_at",
    "harness",
    "executable",
    "argv",
    "cwd",
    "profile",
    "environment_names",
    "slot",
    "slot_project",
    "slot_isolation",
    "workspace_label",
    "native_session",
];
const MAX_ARGUMENTS: usize = 512;
const MAX_ARGUMENT_BYTES: usize = 64 << 10;
const MAX_TOTAL_ARGUMENT_BYTES: usize = 256 << 10;

/// The fields of one new spec; `native_session` is `(value, source)`.
pub(super) struct SpecFields<'a> {
    pub source: &'a str,
    pub captured_at: f64,
    pub harness: &'a str,
    pub executable: Option<&'a str>,
    pub argv: &'a [String],
    pub cwd: &'a str,
    pub profile: Option<&'a str>,
    pub environment_names: &'a [String],
    pub slot: Option<&'a str>,
    pub slot_project: Option<&'a str>,
    pub slot_isolation: Option<&'a str>,
    pub workspace_label: Option<&'a str>,
    pub native_session: Option<(&'a str, &'a str)>,
}

fn text(value: &Value) -> bool {
    value.as_str().is_some_and(valid_metadata_text)
}

fn optional_text(value: &Value) -> bool {
    value.is_null() || text(value)
}

pub(super) fn bounded_arguments(argv: &[String]) -> bool {
    argv.len() <= MAX_ARGUMENTS
        && argv
            .iter()
            .all(|item| valid_metadata_text(item) && item.len() <= MAX_ARGUMENT_BYTES)
        && argv.iter().map(String::len).sum::<usize>() <= MAX_TOTAL_ARGUMENT_BYTES
}

fn kind(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Prove the spec's exact shape for a record of `harness`, or refuse.
pub(super) fn validate(value: &Value, harness: &str, label: &str) -> Result<()> {
    let refuse = |detail: &str| fail(format!("invalid launch spec ({detail}) in {label}"));
    let Some(spec) = value.as_object() else {
        return Err(refuse("fields"));
    };
    if spec.len() != KEYS.len() || KEYS.iter().any(|key| !spec.contains_key(*key)) {
        return Err(refuse("fields"));
    }
    if spec["schema"] != SCHEMA
        || !spec["source"]
            .as_str()
            .is_some_and(|source| SOURCES.contains(&source))
    {
        return Err(refuse("schema or source"));
    }
    if !spec["captured_at"]
        .as_f64()
        .is_some_and(|captured| captured.is_finite() && captured >= 0.0)
    {
        return Err(refuse("capture time"));
    }
    if spec["harness"] != harness || !kind(harness) {
        return Err(refuse("harness"));
    }
    if !optional_text(&spec["executable"]) {
        return Err(refuse("executable"));
    }
    let argv = spec["argv"].as_array().and_then(|items| {
        items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
    });
    if !argv.as_deref().is_some_and(bounded_arguments) {
        return Err(refuse("argv"));
    }
    if !spec["cwd"]
        .as_str()
        .is_some_and(|cwd| valid_metadata_text(cwd) && Path::new(cwd).is_absolute())
    {
        return Err(refuse("cwd"));
    }
    if !(spec["profile"].is_null() || spec["profile"].as_str().is_some_and(name_pattern)) {
        return Err(refuse("profile"));
    }
    let names = spec["environment_names"].as_array().and_then(|items| {
        items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
    });
    if !names.is_some_and(|names| {
        names.iter().all(|name| valid_environment_name(name))
            && names.windows(2).all(|pair| pair[0] < pair[1])
    }) {
        return Err(refuse("environment names"));
    }
    let slot = &spec["slot"];
    if !optional_text(slot) {
        return Err(refuse("slot"));
    }
    if !(spec["slot_project"].is_null()
        || spec["slot_project"].as_str().is_some_and(|project| {
            valid_metadata_text(project) && Path::new(project).is_absolute()
        }))
    {
        return Err(refuse("slot project"));
    }
    if !(spec["slot_isolation"].is_null()
        || spec["slot_isolation"]
            .as_str()
            .is_some_and(|isolation| SLOT_ISOLATIONS.contains(&isolation)))
    {
        return Err(refuse("slot isolation"));
    }
    if slot.is_null() && !(spec["slot_project"].is_null() && spec["slot_isolation"].is_null()) {
        return Err(refuse("slot policy without a slot"));
    }
    if !(spec["workspace_label"].is_null()
        || spec["workspace_label"]
            .as_str()
            .is_some_and(|label| valid_metadata_text(label) && label.len() <= 128))
    {
        return Err(refuse("workspace label"));
    }
    let native = &spec["native_session"];
    if !native.is_null() {
        let valid = native.as_object().is_some_and(|object| {
            object.len() == 3
                && object.get("agent").and_then(Value::as_str) == Some(harness)
                && object.get("value").is_some_and(text)
                && object
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| NATIVE_SOURCES.contains(&source))
        });
        if !valid {
            return Err(refuse("native session"));
        }
    }
    Ok(())
}

/// Build one validated spec.
pub(super) fn build(fields: SpecFields<'_>) -> Result<Value> {
    let names = fields
        .environment_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let spec = json!({
        "schema": SCHEMA,
        "source": fields.source,
        "captured_at": fields.captured_at,
        "harness": fields.harness,
        "executable": fields.executable,
        "argv": fields.argv,
        "cwd": fields.cwd,
        "profile": fields.profile,
        "environment_names": names,
        "slot": fields.slot,
        "slot_project": fields.slot_project,
        "slot_isolation": fields.slot_isolation,
        "workspace_label": fields.workspace_label,
        "native_session": fields.native_session.map(|(value, source)| json!({
            "agent": fields.harness,
            "value": value,
            "source": source,
        })),
    });
    validate(&spec, fields.harness, "a new launch spec")?;
    Ok(spec)
}

/// Separate the conversation selectors from the rest, refusing anything ambiguous; each
/// harness's selectors live with its adapter (#574 harness-adapters).
fn split(harness: &str, argv: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    crate::harness::adapter(harness)
        .and_then(|adapter| adapter.split_conversation(argv))
        .unwrap_or_else(|| {
            Err(format!(
                "launch specs replay Claude, Codex and Muse conversations, not {harness:?}"
            ))
        })
        .map_err(fail)
}

/// The one conversation the argument vector selects, or `None`; refuse a conflict.
pub(super) fn argv_conversation(harness: &str, argv: &[String]) -> Result<Option<String>> {
    let (values, _) = split(harness, argv)?;
    if values.iter().collect::<BTreeSet<_>>().len() > 1 {
        return Err(fail("recorded arguments select more than one conversation"));
    }
    Ok(values.into_iter().next())
}

/// The recorded arguments with every selector replaced by one explicit resume.
pub(super) fn replay_arguments(
    harness: &str,
    argv: &[String],
    conversation: &str,
) -> Result<Vec<String>> {
    if !valid_metadata_text(conversation) {
        return Err(fail("a replay needs a nonempty native conversation id"));
    }
    let (_, rest) = split(harness, argv)?;
    let adapter = crate::harness::adapter(harness).expect("split accepted the harness");
    Ok(adapter.replay_arguments(rest, conversation))
}

/// The recorded arguments with every selector replaced by the start of a new conversation
/// `conversation`; refused for a harness that cannot be given a new conversation's ID.
pub(super) fn fresh_arguments(
    harness: &str,
    argv: &[String],
    conversation: &str,
) -> Result<Vec<String>> {
    if !valid_metadata_text(conversation) {
        return Err(fail(
            "a fresh start needs a nonempty native conversation id",
        ));
    }
    let (_, rest) = split(harness, argv)?;
    let adapter = crate::harness::adapter(harness).expect("split accepted the harness");
    adapter.fresh_arguments(rest, conversation).ok_or_else(|| {
        fail(format!(
            "{harness} cannot be started on a conversation ID agentctl chooses, so it cannot be revived fresh"
        ))
    })
}

/// Is the recorded program the harness itself, so Herdr's kind launch reproduces it?
pub(super) fn program_matches(harness: &str, executable: Option<&str>) -> bool {
    executable.is_none_or(|executable| {
        let name = Path::new(executable)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        name.strip_suffix(" (deleted)").unwrap_or(name) == harness
    })
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    /// Read a spec from the pinned harness's own command line, or `None`.
    ///
    /// The conversation is, in order: one the caller names, the one Herdr reports, the one
    /// agentctl recorded at start, and the one the command line selects. A caller's
    /// conversation that contradicts Herdr's report is refused.
    pub(super) fn captured_launch_spec(
        &self,
        record: &AgentRecord,
        identity: Option<&CustomProcessIdentity>,
        source: &str,
        workspace_label: Option<&str>,
        session: Option<&str>,
    ) -> Result<Option<Value>> {
        let Some(identity) = identity else {
            return Ok(None);
        };
        let Some(argv) = self.client.process_command_line(identity) else {
            return Ok(None);
        };
        if argv.is_empty() || !bounded_arguments(&argv[1..]) {
            return Ok(None);
        }
        let reported = record.observed_session_value();
        if let (Some(session), Some(reported)) = (session, reported) {
            if session != reported {
                return Err(fail(format!(
                    "session {session:?} contradicts the session Herdr reports for {:?}",
                    record.name
                )));
            }
        }
        let selected = if matches!(record.harness.as_str(), "claude" | "codex" | "muse") {
            argv_conversation(&record.harness, &argv[1..]).unwrap_or(None)
        } else {
            None
        };
        let recorded = record
            .native_session
            .as_ref()
            .map(|native| native.value.as_str());
        let native = session
            .map(|value| (value, "caller"))
            .or_else(|| reported.map(|value| (value, "observed")))
            .or_else(|| recorded.map(|value| (value, "asserted")))
            .or_else(|| selected.as_deref().map(|value| (value, "argv")));
        build(SpecFields {
            source,
            captured_at: unix_seconds(),
            harness: &record.harness,
            executable: Some(&argv[0]),
            argv: &argv[1..],
            cwd: &record.cwd,
            profile: record.profile.as_deref(),
            environment_names: &record.environment_names,
            slot: record.slot.as_deref(),
            slot_project: record.slot_project.as_deref(),
            slot_isolation: record.slot_isolation.as_deref(),
            workspace_label,
            native_session: native,
        })
        .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn replay_replaces_every_claude_selector_with_one_resume() {
        let argv = strings(&[
            "--dangerously-skip-permissions",
            "--session-id",
            "old",
            "--fork-session",
            "--model",
            "opus",
        ]);
        assert_eq!(
            argv_conversation("claude", &argv).unwrap().as_deref(),
            Some("old")
        );
        assert_eq!(
            replay_arguments("claude", &argv, "new").unwrap(),
            strings(&[
                "--resume",
                "new",
                "--dangerously-skip-permissions",
                "--model",
                "opus"
            ])
        );
        assert_eq!(
            argv_conversation("claude", &strings(&["--resume=one", "-rtwo"]))
                .unwrap_err()
                .to_string(),
            "recorded arguments select more than one conversation"
        );
        assert!(replay_arguments("claude", &strings(&["--resume", "--model", "x"]), "s").is_err());
        assert!(replay_arguments("claude", &strings(&["-cfoo"]), "s").is_err());
        assert_eq!(
            replay_arguments("claude", &strings(&["--continue", "-c"]), "s").unwrap(),
            strings(&["--resume", "s"])
        );
    }

    #[test]
    fn replay_keeps_codex_and_muse_selector_positions() {
        let codex = strings(&["resume", "old", "--no-alt-screen", "--model", "m"]);
        assert_eq!(
            replay_arguments("codex", &codex, "new").unwrap(),
            strings(&["resume", "new", "--no-alt-screen", "--model", "m"])
        );
        assert!(replay_arguments("codex", &strings(&["resume", "--last"]), "s").is_err());
        assert!(
            replay_arguments("codex", &strings(&["--model", "m", "resume", "x"]), "s").is_err()
        );
        assert_eq!(
            replay_arguments("muse", &strings(&["--model", "m", "resume", "old"]), "new").unwrap(),
            strings(&["--model", "m", "resume", "new"])
        );
        assert!(replay_arguments("bash", &[], "s").is_err());
    }

    #[test]
    fn specs_are_strict_and_bounded() {
        let argv = strings(&["--model", "m"]);
        let names = strings(&["B", "A", "A"]);
        let spec = build(SpecFields {
            source: "adopt",
            captured_at: 1.5,
            harness: "codex",
            executable: Some("/usr/bin/codex"),
            argv: &argv,
            cwd: "/work",
            profile: None,
            environment_names: &names,
            slot: None,
            slot_project: None,
            slot_isolation: None,
            workspace_label: Some("project"),
            native_session: Some(("conversation", "argv")),
        })
        .unwrap();
        assert_eq!(spec["environment_names"], json!(["A", "B"]));
        validate(&spec, "codex", "test").unwrap();
        assert!(validate(&spec, "claude", "test").is_err());
        for (key, value) in [
            ("schema", json!("agentctl-launch-spec/v2")),
            ("source", json!("guess")),
            ("captured_at", json!(-1)),
            ("argv", json!([""])),
            ("cwd", json!("relative")),
            ("environment_names", json!(["B", "A"])),
            ("slot_isolation", json!("root")),
            (
                "native_session",
                json!({"agent": "codex", "value": "x", "source": "rumour"}),
            ),
        ] {
            let mut changed = spec.clone();
            changed[key] = value;
            assert!(
                validate(&changed, "codex", "test").is_err(),
                "accepted {key}"
            );
        }
        let mut extra = spec.clone();
        extra["unexpected"] = json!(true);
        assert!(validate(&extra, "codex", "test").is_err());
        let long = vec!["x".repeat(MAX_ARGUMENT_BYTES + 1)];
        assert!(!bounded_arguments(&long));
        assert!(program_matches(
            "claude",
            Some("/usr/local/bin/claude_code/claude (deleted)")
        ));
        assert!(!program_matches("claude", Some("/usr/bin/node")));
    }
}
