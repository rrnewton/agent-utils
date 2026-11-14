//! Read-only tool use for Discuss follow-ups. `#1356 discuss-readonly-tools`.
//!
//! The first summary stays one tool-free call ([`super::Summarizer::complete_prompt`]).
//! A follow-up may instead ask the model to pull more context, but only through
//! [`DISCUSS_TOOL_NAMES`]: a read-only subset of the `crate::mcp` manifest, executed
//! in-process against this server's cache (never a second service, never upstream).
//! Writes are refused by name here, before any executor runs, so a model that asks
//! for `post_reply` or `mark_read` hears a refusal as its tool result and can still
//! answer from what it has. The loop is bounded: at most [`MAX_TOOL_ROUNDS`] rounds
//! of tool calls, then one final tool-free step answers from what was gathered.

use async_trait::async_trait;
use serde_json::Value;

use super::SummaryError;
use crate::model::ChannelInfo;

/// The read-only tools a Discuss follow-up may call, by manifest name.
pub const DISCUSS_TOOL_NAMES: [&str; 6] = [
    "find_message",
    "read_page",
    "read_message",
    "digest_channel",
    "fetch_document",
    "search",
];

/// At most this many rounds of tool calls in one follow-up, then a final
/// tool-free step answers from what was gathered.
pub const MAX_TOOL_ROUNDS: usize = 4;

/// Whether `name` is a Discuss follow-up tool.
#[must_use]
pub fn is_discuss_tool(name: &str) -> bool {
    DISCUSS_TOOL_NAMES.contains(&name)
}

/// One tool offered to the model, in chat-completions shape.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    /// Manifest name.
    pub name: String,
    /// What it does, for the model.
    pub description: String,
    /// JSON schema of its arguments.
    pub parameters: Value,
}

impl ToolSpec {
    /// The `tools` entry for this tool.
    #[must_use]
    pub fn chat_json(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }
}

/// The tool specs for `channels`: the read-only manifest subset, in manifest order.
#[must_use]
pub fn discuss_tool_specs(channels: &[ChannelInfo]) -> Vec<ToolSpec> {
    crate::mcp::tool_manifest(channels, crate::send_approval::Approval::Ask)
        .into_iter()
        .filter(|tool| is_discuss_tool(tool.name) && !tool.mutates && tool.mcp_exposed)
        .map(|tool| ToolSpec {
            name: tool.name.to_owned(),
            description: tool.description,
            parameters: tool.arguments,
        })
        .collect()
}

/// One tool call the model asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    /// Call id the tool result answers to.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments, as the JSON text the model sent.
    pub arguments: String,
}

/// What one model step answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelStep {
    /// Final text.
    Text(String),
    /// Tool calls to execute before the next step.
    Calls(Vec<ToolCall>),
}

/// One message in a tool-loop conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolMessage {
    /// The follow-up prompt (window, transcript, question).
    User(String),
    /// A text answer from the model.
    Assistant(String),
    /// Tool calls from the model.
    AssistantCalls(Vec<ToolCall>),
    /// One tool's result, answering `call_id`.
    ToolResult {
        /// The call it answers.
        call_id: String,
        /// What the tool said, or why it was refused.
        content: String,
    },
}

/// A model that can take one tool-aware step.
#[async_trait]
pub trait DiscussModel: Send + Sync {
    /// One step over `history`; `tools` empty means no tool may be called.
    ///
    /// # Errors
    ///
    /// [`SummaryError`] when the backend cannot answer.
    async fn discuss_step(
        &self,
        system: &str,
        history: &[ToolMessage],
        tools: &[ToolSpec],
    ) -> Result<ModelStep, SummaryError>;
}

/// Something that executes one allowed tool call against the cache.
#[async_trait]
pub trait DiscussExecutor: Send + Sync {
    /// Run `name` with `arguments`; the text the model reads next.
    ///
    /// # Errors
    ///
    /// A model-facing sentence when the call cannot be served from the cache.
    async fn execute_discuss_tool(&self, name: &str, arguments: Value) -> Result<String, String>;
}

/// Run one follow-up: `prompt` to the model with `tools` offered, tool results
/// fed back, at most [`MAX_TOOL_ROUNDS`] tool rounds and one final tool-free step.
///
/// A call outside [`DISCUSS_TOOL_NAMES`] is never executed: its tool result is a
/// refusal naming the allowlist, and the loop continues so the model can answer
/// without it.
///
/// # Errors
///
/// [`SummaryError`] when a model step fails, or the final tool-free step still
/// asks for a tool.
pub async fn run_discuss_loop(
    model: &dyn DiscussModel,
    executor: &dyn DiscussExecutor,
    system: &str,
    prompt: &str,
    tools: &[ToolSpec],
) -> Result<String, SummaryError> {
    let mut history = vec![ToolMessage::User(prompt.to_owned())];
    for _round in 0..MAX_TOOL_ROUNDS {
        match model.discuss_step(system, &history, tools).await? {
            ModelStep::Text(text) => return Ok(text),
            ModelStep::Calls(calls) => {
                history.push(ToolMessage::AssistantCalls(calls.clone()));
                for call in &calls {
                    let content = if !is_discuss_tool(&call.name) {
                        format!(
                            "refused: {} is not available to Discuss follow-ups. Only these \
                             read-only tools may be called: {}. Nothing was changed.",
                            call.name,
                            DISCUSS_TOOL_NAMES.join(", ")
                        )
                    } else {
                        let arguments =
                            serde_json::from_str::<Value>(&call.arguments).unwrap_or(Value::Null);
                        executor
                            .execute_discuss_tool(&call.name, arguments)
                            .await
                            .unwrap_or_else(|error| format!("not served from the cache: {error}"))
                    };
                    history.push(ToolMessage::ToolResult {
                        call_id: call.id.clone(),
                        content,
                    });
                }
            }
        }
    }
    // Bound reached: one last step with no tools offered, answering from gathered results.
    match model.discuss_step(system, &history, &[]).await? {
        ModelStep::Text(text) => Ok(text),
        ModelStep::Calls(_) => Err(SummaryError::Shape(
            "the model still asked for a tool when none was offered".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct ScriptedModel {
        steps: Mutex<Vec<ModelStep>>,
        seen_tools: Mutex<Vec<usize>>,
    }

    impl ScriptedModel {
        fn new(steps: Vec<ModelStep>) -> Self {
            Self {
                steps: Mutex::new(steps),
                seen_tools: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> usize {
            self.seen_tools.lock().expect("tools").len()
        }
    }

    #[async_trait]
    impl DiscussModel for ScriptedModel {
        async fn discuss_step(
            &self,
            _system: &str,
            _history: &[ToolMessage],
            tools: &[ToolSpec],
        ) -> Result<ModelStep, SummaryError> {
            self.seen_tools.lock().expect("tools").push(tools.len());
            let mut steps = self.steps.lock().expect("steps");
            if steps.is_empty() {
                Ok(ModelStep::Text("final".to_owned()))
            } else {
                Ok(steps.remove(0))
            }
        }
    }

    struct RecordingExecutor {
        executed: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl DiscussExecutor for RecordingExecutor {
        async fn execute_discuss_tool(
            &self,
            name: &str,
            _arguments: Value,
        ) -> Result<String, String> {
            self.executed
                .lock()
                .expect("executed")
                .push(name.to_owned());
            Ok(format!("cached result for {name}"))
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: "{}".to_owned(),
        }
    }

    fn tools() -> Vec<ToolSpec> {
        DISCUSS_TOOL_NAMES
            .iter()
            .map(|name| ToolSpec {
                name: (*name).to_owned(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            })
            .collect()
    }

    #[tokio::test]
    async fn a_write_shaped_call_is_refused_and_never_executed() {
        let model = ScriptedModel::new(vec![
            ModelStep::Calls(vec![call("1", "post_reply"), call("2", "mark_read")]),
            ModelStep::Text("answered without writing".to_owned()),
        ]);
        let executor = RecordingExecutor {
            executed: Mutex::new(Vec::new()),
        };
        let text = run_discuss_loop(&model, &executor, "sys", "q", &tools())
            .await
            .expect("loop answers");
        assert_eq!(text, "answered without writing");
        assert!(
            executor.executed.lock().expect("executed").is_empty(),
            "no write-shaped call may reach the executor"
        );
    }

    #[tokio::test]
    async fn the_loop_stops_at_the_bound_and_answers_from_what_was_gathered() {
        let mut steps: Vec<ModelStep> = (0..MAX_TOOL_ROUNDS)
            .map(|n| ModelStep::Calls(vec![call(&n.to_string(), "search")]))
            .collect();
        steps.push(ModelStep::Text("gathered answer".to_owned()));
        let model = ScriptedModel::new(steps);
        let executor = RecordingExecutor {
            executed: Mutex::new(Vec::new()),
        };
        let text = run_discuss_loop(&model, &executor, "sys", "q", &tools())
            .await
            .expect("loop answers at the bound");
        assert_eq!(text, "gathered answer");
        assert_eq!(
            executor.executed.lock().expect("executed").len(),
            MAX_TOOL_ROUNDS,
            "tool rounds stop at the bound"
        );
        assert_eq!(
            model.calls(),
            MAX_TOOL_ROUNDS + 1,
            "one final tool-free step after the bound"
        );
        assert_eq!(
            model.seen_tools.lock().expect("tools")[MAX_TOOL_ROUNDS],
            0,
            "the final step offers no tools"
        );
    }

    #[tokio::test]
    async fn an_allowed_call_is_executed_and_its_result_feeds_the_answer() {
        let model = ScriptedModel::new(vec![
            ModelStep::Calls(vec![call("1", "read_message")]),
            ModelStep::Text("the cached message says noon".to_owned()),
        ]);
        let executor = RecordingExecutor {
            executed: Mutex::new(Vec::new()),
        };
        let text = run_discuss_loop(&model, &executor, "sys", "q", &tools())
            .await
            .expect("loop answers");
        assert_eq!(text, "the cached message says noon");
        assert_eq!(
            executor.executed.lock().expect("executed").as_slice(),
            ["read_message"]
        );
    }

    #[test]
    fn the_specs_are_exactly_the_read_only_subset() {
        let specs = discuss_tool_specs(&[]);
        let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "digest_channel",
                "read_page",
                "find_message",
                "search",
                "read_message",
                "fetch_document"
            ]
        );
        for spec in &specs {
            let json = spec.chat_json();
            assert_eq!(json["type"], "function");
            assert_eq!(json["function"]["name"], spec.name);
        }
    }
}
