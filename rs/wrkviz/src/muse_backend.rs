//! Strict, non-interactive Muse CLI adapter for structured summary work.
//!
//! The default summary backend runs the headless `muse exec` once per backend batch, with the
//! answer shaped by `--output-schema` and every tool, rule, reminder and session log turned
//! off. The prompt and the schema are the ones the Claude and Codex backends receive, so the
//! answers fill the same summary cache entries.

use std::path::Path;

use serde_json::Value;

use crate::json::dump_canonical;
use crate::summarize_core::{run_process, BatchFailure, ClaudeResult, TIMED_OUT_EXIT};
use crate::token_usage::TokenUsage;

/// The model the muse backend uses for phase summaries, agent names and the project overview.
/// Spark 1.4 since #1275 muse-ember-summarizer (it matched or beat 1.3 on every summary kind);
/// roll back by naming "muse-spark-1.3-internal" here, or per run with --model muse-spark-1.3-internal.
pub const DEFAULT_MUSE_MODEL: &str = "muse-spark-1.4-internal";

/// The model the muse backend uses for calendar rollups, which gain most from a larger model.
pub const DEFAULT_MUSE_ROLLUP_MODEL: &str = "watermelon-full-preview";

/// Per-kind defaults (#1324 per-kind-summarizer-model). Phrase and paragraph are written
/// by the same phase call, so they share the phase default; the daily rollup uses the
/// rollup default. Per run, --model overrides phrase/paragraph and --rollup-model daily.
pub const DEFAULT_MUSE_PHRASE_MODEL: &str = DEFAULT_MUSE_MODEL;
/// The per-kind default for paragraph summaries: the phase default (see above).
pub const DEFAULT_MUSE_PARAGRAPH_MODEL: &str = DEFAULT_MUSE_MODEL;
/// The per-kind default for daily rollups: the rollup default (see above).
pub const DEFAULT_MUSE_DAILY_MODEL: &str = DEFAULT_MUSE_ROLLUP_MODEL;

/// The muse model for one summary kind ("phrase", "paragraph" or "daily"): the CLI
/// override for the kind's call wins over the per-kind default.
pub fn muse_model_for_kind(
    kind: &str,
    phase_model: Option<&str>,
    rollup_model: Option<&str>,
) -> Result<String, String> {
    match kind {
        "phrase" | "paragraph" => Ok(phase_model.unwrap_or(DEFAULT_MUSE_MODEL).to_string()),
        "daily" => Ok(rollup_model
            .unwrap_or(DEFAULT_MUSE_ROLLUP_MODEL)
            .to_string()),
        other => Err(format!(
            "unknown summary kind {other:?}: use 'phrase', 'paragraph' or 'daily'"
        )),
    }
}

/// The muse reasoning effort when the run names none.
pub const DEFAULT_MUSE_REASONING_EFFORT: &str = "medium";

/// `muse exec` flags that keep one call a pure prompt-to-JSON transformation: no tools, no
/// workspace writes, no web, no reminders, no questions back, no session log on disk, and none
/// of the operator's personal rules or skills.
const ISOLATION_FLAGS: &[&str] = &[
    "--preset",
    "empty",
    "--disable-shell",
    "--disable-write",
    "--disable-web-tools",
    "--disable-reminders",
    "--disable-request-user-input",
    "--no-session-log",
    "--no-foreign-personal-context",
    "--disable-muse-llm-rules",
];

/// What one `muse exec --json` event stream reported.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MuseRunEvents {
    /// The `run_terminal` state, such as `completed`.
    pub terminal: Option<String>,
    /// The `run_terminal` reason.
    pub reason: Option<String>,
    /// The final answer text.
    pub text: Option<String>,
    /// Token usage summed over every model the run used.
    pub usage: Option<TokenUsage>,
}

/// Read the terminal state, final text and token usage from `muse exec --json` output.
///
/// Each line is one JSON event; its `payload.kind` is `run_terminal` (with `terminal`,
/// `reason` and the final `text`) or `run_usage` (with per-model `input_tokens`,
/// `cached_tokens`, `output_tokens` and `reasoning_tokens`). Other events and lines that are
/// not JSON are ignored. `input_tokens` already counts the cached part.
pub fn parse_muse_events(stdout: &str) -> Result<MuseRunEvents, String> {
    let mut events = MuseRunEvents::default();
    let text_field = |payload: &serde_json::Map<String, Value>, key: &str| {
        payload.get(key).and_then(Value::as_str).map(str::to_string)
    };
    for line in stdout.lines() {
        let Ok(Value::Object(event)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(Value::Object(payload)) = event.get("payload") else {
            continue;
        };
        match payload.get("kind").and_then(Value::as_str) {
            Some("run_terminal") => {
                events.terminal = text_field(payload, "terminal");
                events.reason = text_field(payload, "reason");
                events.text = text_field(payload, "text");
            }
            Some("run_usage") => {
                let Some(Value::Array(models)) = payload.get("models") else {
                    return Err("muse run_usage event has no models list".to_string());
                };
                let keys = [
                    "input_tokens",
                    "cached_tokens",
                    "output_tokens",
                    "reasoning_tokens",
                ];
                let mut totals = [0i64; 4];
                for model in models {
                    let Value::Object(model) = model else {
                        return Err("muse run_usage model entry is not an object".to_string());
                    };
                    for (total, key) in totals.iter_mut().zip(keys) {
                        let value = match model.get(key) {
                            None => 0,
                            Some(value) => value
                                .as_u64()
                                .and_then(|n| i64::try_from(n).ok())
                                .ok_or_else(|| {
                                    format!("muse run_usage {key} is not a non-negative integer")
                                })?,
                        };
                        *total += value;
                    }
                }
                let [input, cached, output, reasoning] = totals;
                events.usage = Some(TokenUsage::new(
                    input.max(cached),
                    cached,
                    0,
                    output.max(reasoning),
                    reasoning,
                )?);
            }
            _ => {}
        }
    }
    Ok(events)
}

fn failure(message: String, usage: Option<TokenUsage>) -> BatchFailure {
    BatchFailure {
        message,
        usage,
        backend_output: None,
    }
}

/// Run `muse exec` once with every tool disabled and require schema-shaped JSON.
///
/// The prompt and the schema go in files under `cwd` (a private temporary directory), so
/// neither appears in a process listing. There is deliberately no model or backend fallback:
/// a process, terminal-state, schema or accounting failure aborts the batch before any cache
/// artifact is published.
pub(crate) fn run_muse_json(
    command: &[String],
    prompt: &str,
    schema: &Value,
    model: &str,
    reasoning_effort: Option<&str>,
    cwd: &Path,
) -> Result<ClaudeResult, BatchFailure> {
    if command.is_empty() {
        return Err(failure("muse command must not be empty".to_string(), None));
    }
    // Keep the complete schema in the summarization layer for validation, as the Claude
    // backend does, and hand the CLI the schema without the dialect declaration.
    let mut cli_schema = schema.as_object().cloned().unwrap_or_default();
    cli_schema.remove("$schema");
    let schema_path = cwd.join("output-schema.json");
    let prompt_path = cwd.join("prompt.txt");
    let write = |path: &Path, text: &str| {
        std::fs::write(path, text)
            .map_err(|error| failure(crate::json::os_error(&error, path), None))
    };
    write(&schema_path, &dump_canonical(&Value::Object(cli_schema)))?;
    write(&prompt_path, prompt)?;
    let mut args: Vec<String> = command.to_vec();
    for arg in ["exec", "--json", "--prompt-file"] {
        args.push(arg.into());
    }
    args.push(prompt_path.display().to_string());
    args.push("--output-schema".into());
    args.push(schema_path.display().to_string());
    args.push("--model".into());
    args.push(model.into());
    args.push("--reasoning-effort".into());
    args.push(
        reasoning_effort
            .unwrap_or(DEFAULT_MUSE_REASONING_EFFORT)
            .into(),
    );
    args.extend(ISOLATION_FLAGS.iter().map(|flag| (*flag).to_string()));
    let completed = run_process(&args, "", cwd)
        .map_err(|error| failure(format!("could not start muse backend: {error}"), None))?;
    let events = parse_muse_events(&completed.stdout).map_err(|error| {
        failure(
            format!("muse backend reported invalid usage: {error}"),
            None,
        )
    })?;
    if completed.returncode != 0 {
        let words: Vec<&str> = completed.stderr.split_whitespace().collect();
        let joined = words.join(" ");
        let detail: String = {
            let chars: Vec<char> = joined.chars().collect();
            chars[chars.len().saturating_sub(500)..].iter().collect()
        };
        let what = if completed.returncode == TIMED_OUT_EXIT {
            "was stopped at its time limit".to_string()
        } else {
            format!("failed with exit {}", completed.returncode)
        };
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        return Err(failure(
            format!("muse backend {what}{suffix}"),
            events.usage,
        ));
    }
    let Some(usage) = events.usage else {
        return Err(failure("muse backend reported no usage".to_string(), None));
    };
    if events.terminal.as_deref() != Some("completed") {
        let reason = events
            .reason
            .or(events.terminal)
            .unwrap_or_else(|| "no terminal event".to_string());
        return Err(failure(
            format!("muse backend run did not complete: {reason}"),
            Some(usage),
        ));
    }
    let Some(text) = events.text else {
        return Err(failure(
            "muse backend produced no final text".to_string(),
            Some(usage),
        ));
    };
    let structured = match serde_json::from_str::<Value>(&text) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => {
            return Err(failure(
                "muse backend returned malformed structured output: Muse final text: expected an object"
                    .to_string(),
                Some(usage),
            ))
        }
        Err(error) => {
            return Err(failure(
                format!("muse backend returned malformed structured output: {error}"),
                Some(usage),
            ))
        }
    };
    // One canonical JSON representation for the strict schema parsers and the audit trail.
    Ok(ClaudeResult {
        output: dump_canonical(&structured),
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(payload: &str) -> String {
        format!("{{\"seq\": 1, \"payload\": {payload}}}")
    }

    #[test]
    fn events_report_terminal_text_and_summed_usage() {
        let stdout = [
            "not json".to_string(),
            event(r#"{"kind": "assistant_delta", "text": "partial"}"#),
            event(
                r#"{"kind": "run_usage", "models": [
                    {"input_tokens": 100, "cached_tokens": 40, "output_tokens": 7, "reasoning_tokens": 5},
                    {"input_tokens": 20, "cached_tokens": 0, "output_tokens": 3}]}"#
                    .replace('\n', " ")
                    .as_str(),
            ),
            event(r#"{"kind": "run_terminal", "terminal": "completed", "reason": "done", "text": "{\"a\": 1}"}"#),
        ]
        .join("\n");
        let events = parse_muse_events(&stdout).unwrap();
        assert_eq!(events.terminal.as_deref(), Some("completed"));
        assert_eq!(events.text.as_deref(), Some("{\"a\": 1}"));
        assert_eq!(
            events.usage,
            Some(TokenUsage::new(120, 40, 0, 10, 5).unwrap())
        );
    }

    #[test]
    fn model_for_kind_defaults_and_overrides() {
        // #1324 per-kind-summarizer-model: phrase/paragraph share the phase call.
        assert_eq!(
            muse_model_for_kind("phrase", None, None).unwrap(),
            DEFAULT_MUSE_MODEL
        );
        assert_eq!(
            muse_model_for_kind("paragraph", None, None).unwrap(),
            DEFAULT_MUSE_MODEL
        );
        assert_eq!(
            muse_model_for_kind("daily", None, None).unwrap(),
            DEFAULT_MUSE_ROLLUP_MODEL
        );
        assert_eq!(
            muse_model_for_kind("phrase", Some("small-elk-internal"), None).unwrap(),
            "small-elk-internal"
        );
        assert_eq!(
            muse_model_for_kind("daily", None, Some("small-elk-internal")).unwrap(),
            "small-elk-internal"
        );
        assert!(muse_model_for_kind("weekly", None, None).is_err());
    }

    #[test]
    fn events_reject_malformed_usage() {
        let error = parse_muse_events(&event(
            r#"{"kind": "run_usage", "models": [{"cached_tokens": -1}]}"#,
        ))
        .unwrap_err();
        assert_eq!(
            error,
            "muse run_usage cached_tokens is not a non-negative integer"
        );
        let error = parse_muse_events(&event(r#"{"kind": "run_usage"}"#)).unwrap_err();
        assert_eq!(error, "muse run_usage event has no models list");
    }
}
