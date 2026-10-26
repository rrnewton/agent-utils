//! Rust edition of `agentctl mcp` (#545 rust-primary-agentctl-chat-readers):
//! MCP stdio access to the same named sessions and operations as the CLI.
//!
//! The server is a thin protocol shell: each tool call is validated against
//! the operation table and then run through this same executable's CLI, so
//! there is exactly one implementation of every operation. Operations this
//! edition does not implement (the headless-worker commands) surface the
//! CLI's own refusal as an error result, exactly as the CLI prints it.

use std::io::{BufRead, Write};
use std::process::Command;

use serde_json::{json, Map, Value};

struct Operation {
    name: &'static str,
    description: &'static str,
    required: &'static [&'static str],
    arguments: &'static [&'static str],
}

const OPERATIONS: &[Operation] = &[
    Operation {
        name: "start",
        description: "Start a persistent agent; interactive Herdr or headless Herdr/tmux.",
        required: &["name"],
        arguments: &[
            "name", "cwd", "harness", "mode", "backend", "model", "brief", "resume",
        ],
    },
    Operation {
        name: "adopt",
        description:
            "Register an existing identity-checked Herdr agent without owning its runtime.",
        required: &["name", "pane", "workspace", "cwd", "harness"],
        arguments: &["name", "pane", "workspace", "cwd", "harness", "session"],
    },
    Operation {
        name: "send",
        description: "Submit follow-up text, retaining uncertain delivery for reconciliation.",
        required: &["name", "text"],
        arguments: &["name", "text", "message_id", "model", "ready_timeout"],
    },
    Operation {
        name: "list",
        description: "List all named sessions and their capabilities.",
        required: &[],
        arguments: &[],
    },
    Operation {
        name: "status",
        description: "Inspect one session's identity, runtime, and queue.",
        required: &["name"],
        arguments: &["name"],
    },
    Operation {
        name: "read",
        description: "Read terminal text or an explicit headless answer boundary.",
        required: &["name"],
        arguments: &["name", "output", "lines", "since_turn"],
    },
    Operation {
        name: "wait",
        description: "Wait for readiness; this does not prove goal completion.",
        required: &["name"],
        arguments: &["name", "timeout"],
    },
    Operation {
        name: "stop",
        description:
            "Stop an owned session, or safely unregister an adopted one, and archive state.",
        required: &["name"],
        arguments: &["name"],
    },
    Operation {
        name: "move",
        description:
            "Move a running owned native Herdr session into its configured project workspace.",
        required: &["name"],
        arguments: &["name"],
    },
    Operation {
        name: "pause",
        description: "Pause automated input for human interaction.",
        required: &["name"],
        arguments: &["name"],
    },
    Operation {
        name: "resume",
        description: "Resume automated input after human interaction.",
        required: &["name"],
        arguments: &["name"],
    },
    Operation {
        name: "goal",
        description: "Read native goal state when supported, or send a goal instruction.",
        required: &["name"],
        arguments: &["name", "text", "goal_command_json"],
    },
    Operation {
        name: "bind-session",
        description: "Bind a native conversation ID reported by the harness.",
        required: &["name", "session"],
        arguments: &["name", "session", "goal_command_json"],
    },
    Operation {
        name: "drain",
        description: "Deliver pending interactive requests without replaying uncertain input.",
        required: &["name"],
        arguments: &["name", "ready_timeout"],
    },
    Operation {
        name: "migrate",
        description: "Move an idle headless worker while preserving conversation state.",
        required: &["name", "backend"],
        arguments: &["name", "backend", "mode"],
    },
    Operation {
        name: "reset",
        description: "Reset an idle headless worker's conversation.",
        required: &["name"],
        arguments: &["name"],
    },
    Operation {
        name: "repair",
        description: "Repair a live headless worker's terminal presentation.",
        required: &["name"],
        arguments: &["name"],
    },
    Operation {
        name: "attach",
        description: "Focus the verified terminal for direct inspection.",
        required: &["name"],
        arguments: &["name"],
    },
];

const INTEGERS: [&str; 2] = ["lines", "since_turn"];
const NUMBERS: [&str; 2] = ["ready_timeout", "timeout"];
const MAX_LINE_BYTES: usize = 1_048_576;

fn operation(command: &str) -> Option<&'static Operation> {
    OPERATIONS.iter().find(|op| op.name == command)
}

fn tool_schemas() -> Value {
    Value::Array(
        OPERATIONS
            .iter()
            .map(|op| {
                let mut properties = Map::new();
                for key in op.arguments {
                    let kind = if INTEGERS.contains(key) {
                        "integer"
                    } else if NUMBERS.contains(key) {
                        "number"
                    } else {
                        "string"
                    };
                    properties.insert((*key).to_owned(), json!({"type": kind}));
                }
                json!({
                    "name": format!("agent_{}", op.name.replace('-', "_")),
                    "description": op.description,
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": op.required,
                        "properties": properties,
                    }
                })
            })
            .collect(),
    )
}

fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => value.to_string(),
    }
}

fn build_argv(
    command: &str,
    arguments: &Map<String, Value>,
    registry: &str,
    herdr_bin: &str,
) -> Result<Vec<String>, String> {
    let op = operation(command)
        .ok_or_else(|| format!("unknown tool: agent_{}", command.replace('-', "_")))?;
    if arguments
        .keys()
        .any(|key| !op.arguments.contains(&key.as_str()))
    {
        return Err("tool arguments contain unknown fields".to_owned());
    }
    if op.required.iter().any(|key| !arguments.contains_key(*key)) {
        return Err("tool arguments are missing required fields".to_owned());
    }
    let mut argv = vec![
        "--registry".to_owned(),
        registry.to_owned(),
        "--herdr-bin".to_owned(),
        herdr_bin.to_owned(),
        command.to_owned(),
    ];
    for key in ["name", "session"] {
        if let Some(value) = arguments.get(key) {
            let text = value
                .as_str()
                .filter(|text| !text.is_empty() && !text.starts_with('-'))
                .ok_or_else(|| format!("{key} must be a nonempty identifier"))?;
            if key == "name" || command == "bind-session" {
                argv.push(text.to_owned());
            }
        }
    }
    for key in op.arguments {
        if !arguments.contains_key(*key) {
            continue;
        }
        if *key == "name" || *key == "text" || (*key == "session" && command == "bind-session") {
            continue;
        }
        let value = &arguments[*key];
        let valid = if INTEGERS.contains(key) {
            value.as_i64().is_some() || value.as_u64().is_some()
        } else if NUMBERS.contains(key) {
            value.is_number()
        } else {
            value.is_string()
        };
        if !valid {
            return Err(format!("invalid value for {key}"));
        }
        argv.push(format!(
            "--{}={}",
            key.replace('_', "-"),
            render_value(value)
        ));
    }
    if let Some(text) = arguments.get("text") {
        let text = text
            .as_str()
            .ok_or_else(|| "text must be a string".to_owned())?;
        argv.push("--".to_owned());
        argv.push(text.to_owned());
    }
    Ok(argv)
}

fn call_tool(
    name: &str,
    arguments: &Map<String, Value>,
    registry: &str,
    herdr_bin: &str,
) -> Result<Value, String> {
    if !name.starts_with("agent_") {
        return Err(format!("unknown tool: {name}"));
    }
    let command = name
        .strip_prefix("agent_")
        .unwrap_or_default()
        .replace('_', "-");
    if operation(&command).is_none() {
        return Err(format!("unknown tool: {name}"));
    }
    let argv = build_argv(&command, arguments, registry, herdr_bin)?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate this executable: {error}"))?;
    let output = Command::new(executable)
        .args(&argv)
        .output()
        .map_err(|error| format!("cannot run agentctl: {error}"))?;
    let mut rendered = String::from_utf8_lossy(&output.stdout).into_owned();
    rendered.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(json!({
        "content": [{"type": "text", "text": rendered}],
        "isError": !output.status.success(),
    }))
}

fn handle(request: &Value, registry: &str, herdr_bin: &str) -> Result<Option<Value>, String> {
    let object = request
        .as_object()
        .ok_or_else(|| "MCP request must be an object".to_owned())?;
    let identifier = object.get("id").cloned().unwrap_or(Value::Null);
    if identifier.is_null() {
        return Ok(None);
    }
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("expected JSON-RPC 2.0".to_owned());
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "agentctl", "version": env!("CARGO_PKG_VERSION")}
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tool_schemas()}),
        "tools/call" => {
            let params = object
                .get("params")
                .and_then(Value::as_object)
                .ok_or_else(|| "tool call must be an object".to_owned())?;
            let tool = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "tool name must be a string".to_owned())?;
            let empty = Map::new();
            let arguments = match params.get("arguments") {
                None => &empty,
                Some(Value::Object(map)) => map,
                Some(_) => return Err("tool arguments must be an object".to_owned()),
            };
            call_tool(tool, arguments, registry, herdr_bin)?
        }
        other => return Err(format!("unsupported method: {other}")),
    };
    Ok(Some(
        json!({"jsonrpc": "2.0", "id": identifier, "result": result}),
    ))
}

/// Serve bounded JSON-RPC messages on stdio; diagnostics never enter stdout.
pub fn serve(registry: &str, herdr_bin: &str) -> i32 {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    loop {
        let Some(line) = lines.next() else {
            return 0;
        };
        let line = match line {
            Ok(line) => line,
            Err(_) => return 0,
        };
        let mut identifier = Value::Null;
        let response = (|| -> Result<Option<Value>, String> {
            if line.len() > MAX_LINE_BYTES {
                return Err("MCP request exceeds 1 MiB".to_owned());
            }
            let request: Value = serde_json::from_str(&line)
                .map_err(|error| format!("invalid MCP request: {error}"))?;
            if let Some(id) = request.as_object().and_then(|object| object.get("id")) {
                identifier = id.clone();
            }
            handle(&request, registry, herdr_bin)
        })();
        let document = match response {
            Ok(Some(document)) => Some(document),
            Ok(None) => None,
            Err(error) => Some(json!({
                "jsonrpc": "2.0",
                "id": identifier,
                "error": {"code": -32602, "message": error}
            })),
        };
        if let Some(document) = document {
            let text = serde_json::to_string(&document).unwrap_or_default();
            if writeln!(out, "{text}").is_err() || out.flush().is_err() {
                return 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_close_their_arguments() {
        let tools = tool_schemas();
        let tools = tools.as_array().expect("tools");
        assert_eq!(tools.len(), OPERATIONS.len());
        for tool in tools {
            assert_eq!(tool["inputSchema"]["additionalProperties"], json!(false));
        }
    }

    #[test]
    fn argv_routes_session_as_flag_except_bind_session() {
        let mut arguments = Map::new();
        for (key, value) in [
            ("name", "reviewer"),
            ("pane", "w1:p2"),
            ("workspace", "project"),
            ("cwd", "/work"),
            ("harness", "codex"),
            ("session", "thread-1"),
        ] {
            arguments.insert(key.to_owned(), json!(value));
        }
        let argv = build_argv("adopt", &arguments, ".agentctl", "herdr").expect("argv");
        assert!(argv.contains(&"--session=thread-1".to_owned()));
        assert_eq!(argv.last().map(String::as_str), Some("--session=thread-1"));
        let mut bind = Map::new();
        bind.insert("name".to_owned(), json!("reviewer"));
        bind.insert("session".to_owned(), json!("thread-1"));
        let argv = build_argv("bind-session", &bind, ".agentctl", "herdr").expect("argv");
        assert_eq!(argv.last().map(String::as_str), Some("thread-1"));
    }

    #[test]
    fn malformed_calls_refuse() {
        let mut arguments = Map::new();
        arguments.insert("name".to_owned(), json!(true));
        assert!(call_tool("agent_status", &arguments, ".agentctl", "herdr").is_err());
        assert!(call_tool("agent_nope", &Map::new(), ".agentctl", "herdr").is_err());
    }
}
