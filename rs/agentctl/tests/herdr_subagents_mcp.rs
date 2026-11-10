//! B3-8: the Rust `herdr-subagents mcp` server speaks the same stdio
//! JSON-RPC protocol as the Python edition (`py/agentctl/foreign/mcp`),
//! including the `subagent_*` tool schemas, structured tool failures,
//! and the local WebSocket event broadcaster.

#![cfg(target_os = "linux")]

use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn fake_base(test: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("agentctl-mcp-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn mcp_command(base: &Path, ws_port: &str) -> Command {
    let mut command = Command::new(Path::new(env!("CARGO_BIN_EXE_herdr-subagents")));
    command
        .arg("mcp")
        .env("HERDR_SUBAGENTS_HOME", base)
        .env("SUBAGENTS_MCP_WS_PORT", ws_port)
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("SUBAGENTS_BACKEND")
        .env_remove("SUBAGENTS_MODE")
        .env_remove("HERDR_SUBAGENTS_POLICY");
    command
}

fn serve(base: &Path, input: &str) -> Vec<Value> {
    let output = mcp_command(base, "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        })
        .unwrap();
    assert!(
        output.status.success(),
        "mcp exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn request_line(value: Value) -> String {
    format!("{}\n", serde_json::to_string(&value).unwrap())
}

#[test]
fn stdio_protocol_matches_python_contract() {
    let base = fake_base("stdio");
    let mut input = String::new();
    for request in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "subagent_list", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "subagent_status", "arguments": {"name": true}}}),
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "subagent_nope", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "subagent_send", "arguments": {"name": "missing", "message": "PING"}}}),
        json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": {"name": "subagent_down", "arguments": {"name": "missing"}}}),
        json!({"jsonrpc": "2.0", "id": 9, "method": "bogus"}),
        json!({"jsonrpc": "2.0", "id": 10}),
        json!({"jsonrpc": "2.0", "id": 11, "method": "tools/call", "params": {"arguments": {}}}),
    ] {
        input.push_str(&request_line(request));
    }
    input.push_str("{not json\n");
    let replies = serve(&base, &input);
    assert_eq!(replies.len(), 12, "notification gets no reply: {replies:?}");
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        replies[0]["result"]["serverInfo"]["name"],
        "herdr-foreign-subagents"
    );
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "subagent_up",
            "subagent_send",
            "subagent_read",
            "subagent_status",
            "subagent_down",
            "subagent_reset",
            "subagent_recreate_window",
            "subagent_migrate",
            "subagent_list"
        ]
    );
    let up = &tools[0];
    assert_eq!(
        up["inputSchema"]["required"],
        json!(["name", "cwd", "brief"])
    );
    assert_eq!(
        up["inputSchema"]["properties"]["mode"],
        json!({
            "type": ["string", "null"],
            "enum": ["headless", "tui", null],
            "description": "Per-agent override; omitted uses SUBAGENTS_MODE then project_defaults.json."
        })
    );
    assert_eq!(replies[2]["result"], json!({}));
    let listed = &replies[3]["result"];
    assert_eq!(listed["isError"], false);
    assert_eq!(
        listed["structuredContent"],
        json!({"agents": [], "gc_notes": [], "ok": true})
    );
    assert_eq!(
        serde_json::from_str::<Value>(listed["content"][0]["text"].as_str().unwrap()).unwrap(),
        listed["structuredContent"]
    );
    let bad_argument = &replies[4]["result"];
    assert_eq!(bad_argument["isError"], true);
    let payload: Value =
        serde_json::from_str(bad_argument["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        payload,
        json!({"ok": false, "error": {"code": "bad_argument", "message": "'name' must be a string"}})
    );
    let unknown_tool = &replies[5]["result"];
    assert_eq!(unknown_tool["isError"], true);
    let payload: Value =
        serde_json::from_str(unknown_tool["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["error"]["code"], "unknown_tool");
    assert_eq!(
        payload["error"]["message"],
        "unknown MCP tool 'subagent_nope'"
    );
    for reply in [&replies[6], &replies[7]] {
        assert_eq!(reply["result"]["isError"], true);
        let payload: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload["error"]["code"], "unknown_agent", "{payload}");
    }
    assert_eq!(replies[8]["error"]["code"], -32601);
    assert_eq!(replies[8]["error"]["message"], "method not found: bogus");
    assert_eq!(replies[9]["error"]["code"], -32600);
    assert_eq!(replies[10]["error"]["code"], -32602);
    assert_eq!(replies[11]["id"], Value::Null);
    assert_eq!(replies[11]["error"]["code"], -32700);
}

#[test]
fn websocket_broadcaster_accepts_and_broadcasts_events() {
    let base = fake_base("ws");
    let events = base.join("state").join("events.jsonl");
    std::fs::create_dir_all(events.parent().unwrap()).unwrap();
    std::fs::write(&events, "").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child: Child = mcp_command(&base, &port.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _stdin = child.stdin.take().unwrap();
    let mut stream = connect_within(port, Duration::from_secs(10));
    stream
        .write_all(
            b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut headers = String::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !headers.ends_with("\r\n\r\n") {
        assert!(Instant::now() < deadline, "no handshake response");
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte).unwrap();
        headers.push(byte[0] as char);
    }
    assert!(headers.starts_with("HTTP/1.1 101"), "{headers}");
    assert!(
        headers.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "{headers}"
    );
    std::fs::write(
        &events,
        "{\"id\":\"evt-1\",\"type\":\"up\",\"name\":\"alpha\"}\n",
    )
    .unwrap();
    let payload = read_text_frame(&mut reader, Duration::from_secs(10));
    let event: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(event, json!({"id": "evt-1", "type": "up", "name": "alpha"}));
    drop(reader);
    drop(_stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}

fn connect_within(port: u16, timeout: Duration) -> TcpStream {
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                return stream;
            }
            Err(error) => {
                assert!(Instant::now() < deadline, "cannot connect: {error}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn read_text_frame(reader: &mut BufReader<TcpStream>, timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut header = [0_u8; 2];
    reader.read_exact(&mut header).unwrap();
    assert_eq!(header[0], 0x81, "expected a final text frame");
    let mut size = u64::from(header[1] & 0x7f);
    if size == 126 {
        let mut extended = [0_u8; 2];
        reader.read_exact(&mut extended).unwrap();
        size = u64::from(u16::from_be_bytes(extended));
    } else if size == 127 {
        let mut extended = [0_u8; 8];
        reader.read_exact(&mut extended).unwrap();
        size = u64::from_be_bytes(extended);
    }
    assert!(Instant::now() < deadline);
    let mut payload = vec![0_u8; usize::try_from(size).unwrap()];
    reader.read_exact(&mut payload).unwrap();
    payload
}
