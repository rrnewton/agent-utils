//! `vibe-talk <tool>`: one subcommand per MCP tool, generated from the tool manifest.
//! `#1338 vibe-talk-search-cli` (slice 2).
//!
//! The manifest ([`crate::mcp::tool_manifest`], the same source as MCP `tools/list` and
//! `GET /api/v1/agent-tools`) names every tool, its HTTP route, its argument schema, and
//! whether it mutates. This module turns each offered tool (exposed and listed, as
//! `tools/list` offers them) into a subcommand with the same arguments, so an agent with a
//! shell gets exactly the surface an agent with MCP gets, and the two cannot drift: the
//! subcommands are built from the manifest at run time, and a test asserts the sets match.
//! `search` is the one exception: it keeps its hand-written subcommand
//! ([`crate::search_cli`]), whose coverage lines a generic call cannot render.
//!
//! The read/write split is the manifest's: a tool that mutates is called with the WRITE
//! token, and one whose manifest entry asks for approval first (`post_reply` while the
//! owner has ask-before-send on) asks for confirmation unless `--yes` is given. Every
//! subcommand takes `--json`; without it, an answer holding messages prints one line per
//! message with its permalink and vibe-talk deep link, as `search` prints its hits.

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use serde_json::{Map, Value};

use crate::config::Config;
use crate::mcp::{ApprovalMode, ToolDescriptor};
use crate::model::ChannelInfo;

/// Every tool the generated subcommands cover: the manifest's offered tools (exposed and
/// listed, what MCP `tools/list` names) except `search`, whose hand-written subcommand
/// stands in its place. Built from the manifest each call, so a tool added there is a
/// subcommand here without a second edit — the drift test pins that the sets match.
#[must_use]
pub fn generated_tools() -> Vec<ToolDescriptor> {
    crate::mcp::tool_manifest(&[], crate::send_approval::Approval::Ask)
        .into_iter()
        .filter(|tool| tool.mcp_exposed && tool.listed && tool.name != "search")
        .collect()
}

/// A tool's subcommand name: its manifest name with dashes for underscores.
#[must_use]
pub fn subcommand_name(tool: &ToolDescriptor) -> String {
    tool.name.replace('_', "-")
}

/// The generated subcommand names, for the binary's dispatch and the drift test.
#[must_use]
pub fn subcommand_names() -> Vec<String> {
    generated_tools().iter().map(subcommand_name).collect()
}

/// Help for one generated subcommand, from its manifest entry.
#[must_use]
pub fn usage(tool: &ToolDescriptor) -> String {
    let name = subcommand_name(tool);
    let properties = tool.arguments.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = tool
        .arguments
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = format!(
        "vibe-talk {name} — {}\n\nUSAGE:\n    vibe-talk {name}",
        tool.description.replace('\n', " ")
    );
    if let Some(properties) = properties {
        for key in properties.keys() {
            out.push_str(&format!(" [--{} VALUE]", key.replace('_', "-")));
        }
    }
    out.push_str(" [--json] [--yes] [--config PATH] [--server URL] [--token-file PATH]\n");
    if !required.is_empty() {
        out.push_str(&format!("\nRequired: {}\n", required.join(", ")));
    }
    out.push_str(
        "\nSame arguments as the MCP tool of the same name. --json prints the server's \
         answer structured; --yes skips the confirmation a posting tool asks for.\n",
    );
    out
}

/// What a generated subcommand was asked to do.
#[derive(Debug, Default)]
pub struct Options {
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--config-overlay`, in order.
    pub overlays: Vec<PathBuf>,
    /// `--server`.
    pub server: Option<String>,
    /// `--base-url` (deep links).
    pub base_url: Option<String>,
    /// `--token-file`.
    pub token_file: Option<PathBuf>,
    /// `--json`.
    pub json: bool,
    /// `--yes`.
    pub yes: bool,
    /// The tool arguments, by manifest name.
    pub arguments: Map<String, Value>,
}

/// Parse one generated subcommand's arguments against its manifest schema: every property
/// is a `--kebab-case` flag taking a value (a boolean property may stand alone for true),
/// typed as the schema types it. `Ok(None)` means `--help` was printed.
///
/// # Errors
///
/// An unknown flag, a missing value, a mistyped value, or a missing required argument.
pub fn parse(
    tool: &ToolDescriptor,
    args: impl IntoIterator<Item = String>,
) -> anyhow::Result<Option<Options>> {
    let properties = tool
        .arguments
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut options = Options::default();
    let mut args: std::collections::VecDeque<String> = args.into_iter().collect();
    while let Some(arg) = args.pop_front() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{}", usage(tool));
                return Ok(None);
            }
            "--config" => {
                options.config = Some(args.pop_front().context("--config needs a path")?.into())
            }
            "--config-overlay" => options.overlays.push(
                args.pop_front()
                    .context("--config-overlay needs a path")?
                    .into(),
            ),
            "--server" => options.server = Some(args.pop_front().context("--server needs a URL")?),
            "--base-url" => {
                options.base_url = Some(args.pop_front().context("--base-url needs a URL")?)
            }
            "--token-file" => {
                options.token_file = Some(
                    args.pop_front()
                        .context("--token-file needs a path")?
                        .into(),
                )
            }
            "--json" => options.json = true,
            "--yes" => options.yes = true,
            other => {
                let Some(flag) = other.strip_prefix("--") else {
                    anyhow::bail!("unrecognized argument {other:?}\n\n{}", usage(tool));
                };
                let key = flag.replace('-', "_");
                let Some(schema) = properties.get(&key) else {
                    anyhow::bail!("unrecognized argument {other:?}\n\n{}", usage(tool));
                };
                let kind = schema
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("string");
                if kind == "boolean" {
                    let value = match args.front() {
                        Some(text) if !text.starts_with("--") => {
                            let text = text.clone();
                            args.pop_front();
                            parse_typed(&key, "boolean", &text)?
                        }
                        _ => Value::Bool(true),
                    };
                    options.arguments.insert(key, value);
                } else {
                    let value = args
                        .pop_front()
                        .with_context(|| format!("--{flag} needs a value"))?;
                    options
                        .arguments
                        .insert(key.clone(), parse_typed(&key, kind, &value)?);
                }
            }
        }
    }
    if let Some(required) = tool.arguments.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !options.arguments.contains_key(name) {
                anyhow::bail!(
                    "--{} is required\n\n{}",
                    name.replace('_', "-"),
                    usage(tool)
                );
            }
        }
    }
    Ok(Some(options))
}

/// One flag value as its schema type.
///
/// # Errors
///
/// The value does not parse as the named type.
fn parse_typed(key: &str, kind: &str, value: &str) -> anyhow::Result<Value> {
    match kind {
        "integer" => value.parse::<i64>().map(Value::from).with_context(|| {
            format!(
                "--{} takes a whole number, not {value:?}",
                key.replace('_', "-")
            )
        }),
        "number" => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .with_context(|| format!("--{} takes a number, not {value:?}", key.replace('_', "-"))),
        "boolean" => value.parse::<bool>().map(Value::Bool).with_context(|| {
            format!(
                "--{} takes true or false, not {value:?}",
                key.replace('_', "-")
            )
        }),
        _ => Ok(Value::String(value.to_owned())),
    }
}

/// The request a call makes: the manifest path with its `{params}` filled from the
/// arguments, and the arguments that remain — the query for a GET, the body for a POST.
#[must_use]
pub fn request_parts(
    tool: &ToolDescriptor,
    arguments: &Map<String, Value>,
) -> (String, Map<String, Value>) {
    let mut path = tool.path.to_owned();
    let mut rest = arguments.clone();
    for key in arguments.keys() {
        let placeholder = format!("{{{key}}}");
        if path.contains(&placeholder) {
            let value = arguments
                .get(key)
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            path = path.replace(&placeholder, &crate::search_cli::encoded(&value));
            rest.remove(key);
        }
    }
    (path, rest)
}

/// Render an answer without `--json`: one line per message it holds (permalink and
/// vibe-talk deep link, as `search` prints), else the answer pretty-printed.
fn render(base: &str, link_base: &str, channels: &[ChannelInfo], body: &Value) -> String {
    let mut messages: Vec<Value> = Vec::new();
    fn collect(value: &Value, messages: &mut Vec<Value>) {
        match value {
            Value::Object(map) => {
                if map.contains_key("content") && map.contains_key("id") {
                    messages.push(value.clone());
                } else {
                    for inner in map.values() {
                        collect(inner, messages);
                    }
                }
            }
            Value::Array(items) => {
                for inner in items {
                    collect(inner, messages);
                }
            }
            _ => {}
        }
    }
    collect(body, &mut messages);
    if messages.is_empty() {
        return serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
    }
    let parsed: Vec<crate::model::Message> = messages
        .iter()
        .filter_map(|value| serde_json::from_value(value.clone()).ok())
        .collect();
    if parsed.is_empty() {
        return serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
    }
    let _ = base;
    parsed
        .iter()
        .map(|message| crate::search_cli::line_for(link_base, channels, message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run one generated subcommand and return what to print.
///
/// # Errors
///
/// The arguments do not parse, the configuration or token cannot be read, a posting was
/// not confirmed, the server cannot be reached, or it refuses.
pub async fn run(tool: &ToolDescriptor, args: Vec<String>) -> anyhow::Result<String> {
    let Some(options) = parse(tool, args)? else {
        return Ok(String::new());
    };
    let need_config = options.server.is_none() || options.token_file.is_none();
    let config = if need_config {
        Some(
            Config::discover(options.config.clone(), options.overlays.clone()).context(
                "the client reads the server's configuration for its tokens and addresses, as \
             the server does: run it with the server's --config and environment",
            )?,
        )
    } else {
        None
    };
    let token = match &options.token_file {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("reading the token file {}", path.display()))?
            .trim()
            .to_owned(),
        None => {
            let config = config
                .as_ref()
                .context("no token: pass --token-file, or run with the server's --config")?;
            let secret = if tool.mutates {
                &config.auth.write_token
            } else {
                &config.auth.read_token
            };
            secret.expose().to_owned()
        }
    };
    if token.is_empty() {
        anyhow::bail!("the token is empty; check the token file or configuration");
    }
    let base = match &options.server {
        Some(server) => {
            let parsed = url::Url::parse(server)
                .with_context(|| format!("--server {server:?} is not a URL"))?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                anyhow::bail!("--server {server:?} must be an http:// or https:// address");
            }
            server.trim_end_matches('/').to_owned()
        }
        None => {
            let config = config
                .as_ref()
                .context("no server address: give --server or --config")?;
            let local: SocketAddr = crate::pair_link::local_address(config.bind);
            if !crate::pair_link::is_this_host(local.ip()) {
                anyhow::bail!(
                    "the configuration's bind address {} is not an address of this host, and \
                     the token is sent only to this host; run where the server runs, or pass \
                     --server",
                    local.ip()
                );
            }
            format!("http://{local}")
        }
    };
    let link_base = options
        .base_url
        .clone()
        .or_else(|| config.as_ref().and_then(|c| c.public_base_url.clone()))
        .unwrap_or_else(|| base.clone());
    if tool.mutates && tool.approval == ApprovalMode::RequiresApproval && !options.yes {
        eprintln!(
            "{} changes something outside this server ({} {}).",
            subcommand_name(tool),
            tool.method,
            tool.path
        );
        eprint!("Go ahead? [y/N] ");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("reading the confirmation")?;
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            anyhow::bail!("not confirmed; nothing was called (pass --yes to skip the ask)");
        }
    }
    let (path, rest) = request_parts(tool, &options.arguments);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .context("building the HTTP client")?;
    let url = format!("{base}{path}");
    let request = match tool.method {
        "POST" => client.post(url).json(&Value::Object(rest)),
        _ => {
            let query: Vec<(String, String)> = rest
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        match value {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        },
                    )
                })
                .collect();
            client.get(url).query(&query)
        }
    };
    let response = request
        .bearer_auth(&token)
        .send()
        .await
        .context("could not reach the vibe-talk server; is it running?")?;
    let status = response.status();
    let text = response.text().await.context("reading the answer")?;
    if !status.is_success() {
        anyhow::bail!("{} failed ({status}): {text}", subcommand_name(tool));
    }
    let body: Value =
        serde_json::from_str(&text).with_context(|| "the server answered unexpectedly")?;
    if options.json {
        return serde_json::to_string_pretty(&body).context("rendering JSON");
    }
    // Deep links name each hit's provider; the channel list is read only when the answer
    // holds messages, so a plain answer costs no second call.
    let holds_messages = text.contains("\"content\"");
    let channels: Vec<ChannelInfo> = if holds_messages {
        let listed = client
            .get(format!("{base}/api/v1/channels"))
            .bearer_auth(&token)
            .send()
            .await;
        match listed {
            Ok(response) if response.status().is_success() => response
                .json::<Value>()
                .await
                .ok()
                .and_then(|body| serde_json::from_value(body["channels"].clone()).ok())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    Ok(render(&base, &link_base, &channels, &body))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn tool(name: &str) -> ToolDescriptor {
        generated_tools()
            .into_iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} is generated"))
    }

    #[test]
    fn every_offered_tool_has_a_subcommand_and_vice_versa() {
        // The drift test (`#1338 vibe-talk-search-cli` slice 2): the manifest's offered
        // tools and the CLI's subcommands are the same set, with the hand-written `search`
        // standing for its manifest entry.
        let offered: BTreeSet<String> =
            crate::mcp::tool_manifest(&[], crate::send_approval::Approval::Ask)
                .into_iter()
                .filter(|tool| tool.mcp_exposed && tool.listed)
                .map(|tool| tool.name.to_owned())
                .collect();
        let mut cli: BTreeSet<String> = subcommand_names()
            .iter()
            .map(|name| name.replace('-', "_"))
            .collect();
        cli.insert("search".to_owned());
        assert_eq!(offered, cli);
    }

    #[test]
    fn arguments_parse_by_schema_and_required_is_checked() {
        let page = tool("read_page");
        let options = parse(
            &page,
            [
                "--channel-id".to_owned(),
                "C1".to_owned(),
                "--limit".to_owned(),
                "5".to_owned(),
            ],
        )
        .expect("parses")
        .expect("no --help");
        assert_eq!(options.arguments["channel_id"], Value::from("C1"));
        assert_eq!(options.arguments["limit"], Value::from(5));
        assert!(parse(&page, ["--limit".to_owned(), "5".to_owned()]).is_err());
        assert!(parse(
            &page,
            [
                "--channel-id".to_owned(),
                "C1".to_owned(),
                "--nope".to_owned(),
                "1".to_owned()
            ],
        )
        .is_err());
    }

    #[test]
    fn path_parameters_leave_the_rest_for_the_query_or_body() {
        let read = tool("read_message");
        let mut arguments = Map::new();
        arguments.insert("channel_id".to_owned(), Value::from("C 1"));
        arguments.insert("message_id".to_owned(), Value::from("M1"));
        arguments.insert("thread_id".to_owned(), Value::from("T1"));
        let (path, rest) = request_parts(&read, &arguments);
        assert_eq!(path, "/api/v1/channels/C%201/messages/M1");
        assert_eq!(rest.len(), 1);
        assert_eq!(rest["thread_id"], Value::from("T1"));
    }

    #[test]
    fn write_tools_are_the_ones_the_manifest_marks() {
        // The token split follows `mutates`, never a second hand-kept list.
        for name in ["post_reply", "mark_read"] {
            assert!(tool(name).mutates, "{name} must take the write token");
        }
        for name in [
            "recent_activity",
            "list_channels",
            "read_page",
            "find_message",
        ] {
            assert!(!tool(name).mutates, "{name} must take the read token");
        }
    }
}
