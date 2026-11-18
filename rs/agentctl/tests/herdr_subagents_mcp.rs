//! B3-8: the Rust `herdr-subagents mcp` server speaks the same stdio
//! JSON-RPC protocol as the Python edition (`py/agentctl/foreign/mcp`),
//! including the `subagent_*` tool schemas, structured tool failures,
//! and the local WebSocket event broadcaster.

#![cfg(target_os = "linux")]

use std::io::{BufReader, Write};
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
    // The verdict is one broadcast frame carrying an appended event
    // verbatim. Everything before it -- connecting, the handshake, the
    // frame's arrival -- is retried behind one generous backstop: on a
    // loaded host the server's threads can be starved past any short
    // read timeout (the reads used to panic on WouldBlock, and the peer
    // can reset a starved connection), the free port can be taken
    // between probing and binding, and the watcher's first offset read
    // can race the first append and skip it. None of that says anything
    // about the broadcaster, so each attempt starts over with a fresh
    // server and port, and appends further events until one is seen.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last_error = String::new();
    while Instant::now() < deadline {
        match websocket_broadcast_attempt(deadline) {
            Ok(()) => return,
            // Only transport transients retry. A wrong frame, a wrong
            // handshake answer, or a failed server exit is the verdict
            // failing, on the first attempt that produces one.
            Err(AttemptError::Fatal(error)) => {
                panic!("the broadcaster failed its verdict: {error}")
            }
            Err(AttemptError::Transient(error)) => last_error = error,
        }
    }
    panic!("the broadcaster never broadcast an appended event: {last_error}");
}

/// How one attempt failed: a transport transient (starved or reset
/// connection, taken port, skipped append) may retry; anything the
/// server answered with is a verdict and fails at once.
enum AttemptError {
    Transient(String),
    Fatal(String),
}

fn transient(error: impl std::fmt::Display) -> AttemptError {
    AttemptError::Transient(error.to_string())
}

/// One whole attempt: a fresh server on a fresh port must accept the
/// handshake and broadcast one of the events appended to its log.
fn websocket_broadcast_attempt(deadline: Instant) -> Result<(), AttemptError> {
    let base = fake_base("ws");
    let events = base.join("state").join("events.jsonl");
    std::fs::create_dir_all(events.parent().unwrap()).map_err(transient)?;
    std::fs::write(&events, "").map_err(transient)?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(transient)?;
    let port = listener.local_addr().map_err(transient)?.port();
    drop(listener);
    let mut child: Child = mcp_command(&base, &port.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AttemptError::Transient(format!("spawn: {error}")))?;
    let stdin = child.stdin.take().unwrap();
    let result = websocket_broadcast_on(&events, port, deadline);
    drop(stdin);
    // The server ends when its stdin does; bound the reap so a server
    // starved on a loaded host cannot hold the next attempt hostage.
    if result.is_err() {
        let _ = child.kill();
    } else {
        let exit_deadline = Instant::now() + Duration::from_secs(30);
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < exit_deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| AttemptError::Transient(format!("wait: {error}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    match result {
        Ok(()) if output.status.success() => Ok(()),
        Ok(()) => Err(AttemptError::Fatal(format!(
            "server exited {}: {stderr}",
            output.status
        ))),
        Err(error) => match error {
            AttemptError::Fatal(message) => Err(AttemptError::Fatal(format!(
                "{message} (server stderr: {stderr})"
            ))),
            AttemptError::Transient(message) => Err(AttemptError::Transient(format!(
                "{message} (server stderr: {stderr})"
            ))),
        },
    }
}

fn websocket_broadcast_on(events: &Path, port: u16, deadline: Instant) -> Result<(), AttemptError> {
    let mut stream = connect_until(port, deadline)?;
    stream
        .write_all(
            b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .map_err(|error| AttemptError::Transient(format!("handshake write: {error}")))?;
    let mut reader = BufReader::new(stream.try_clone().map_err(transient)?);
    let mut headers = String::new();
    while !headers.ends_with("\r\n\r\n") {
        let mut byte = [0_u8; 1];
        read_exact_retry(&mut reader, &mut byte, deadline)?;
        headers.push(byte[0] as char);
        if headers.len() > 8192 {
            return Err(AttemptError::Fatal(format!(
                "handshake headers run long: {headers:?}"
            )));
        }
    }
    if !headers.starts_with("HTTP/1.1 101")
        || !headers.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    {
        return Err(AttemptError::Fatal(format!(
            "bad handshake response: {headers}"
        )));
    }
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(transient)?;
    // Append events until one is broadcast. The watcher baselines its
    // offset when it starts, so an event appended before that is
    // legitimately never sent; later appends are.
    let mut appended: Vec<String> = Vec::new();
    let mut buffered: Vec<u8> = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(AttemptError::Transient(
                "no broadcast frame arrived".to_owned(),
            ));
        }
        let id = format!("evt-{}", appended.len() + 1);
        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(events)
                .map_err(transient)?;
            use std::io::Write as _;
            writeln!(
                file,
                "{{\"id\":\"{id}\",\"type\":\"up\",\"name\":\"alpha\"}}"
            )
            .map_err(transient)?;
        }
        appended.push(id);
        let cycle_end = (Instant::now() + Duration::from_millis(500)).min(deadline);
        while Instant::now() < cycle_end {
            if let Some(payload) = poll_frame(&mut reader, &mut buffered)? {
                let event: Value = serde_json::from_slice(&payload).map_err(transient)?;
                let id = event["id"].as_str().unwrap_or_default();
                if appended.iter().any(|sent| sent == id)
                    && event == json!({"id": id, "type": "up", "name": "alpha"})
                {
                    return Ok(());
                }
                return Err(AttemptError::Fatal(format!(
                    "unexpected broadcast frame: {event}"
                )));
            }
        }
    }
}

fn connect_until(port: u16, deadline: Instant) -> Result<TcpStream, AttemptError> {
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .map_err(transient)?;
                return Ok(stream);
            }
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(AttemptError::Transient(format!("cannot connect: {error}")));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Fill `target` from `reader`, retrying the transient timeouts a
/// starved peer causes instead of panicking on them.
fn read_exact_retry(
    reader: &mut BufReader<TcpStream>,
    target: &mut [u8],
    deadline: Instant,
) -> Result<(), AttemptError> {
    use std::io::Read as _;
    let mut filled = 0;
    while filled < target.len() {
        match reader.read(&mut target[filled..]) {
            Ok(0) => return Err(AttemptError::Transient("connection closed".to_owned())),
            Ok(read) => filled += read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(AttemptError::Transient(format!("read timed out: {error}")));
                }
            }
            Err(error) => return Err(AttemptError::Transient(format!("read: {error}"))),
        }
    }
    Ok(())
}

/// One non-blocking poll for a complete text frame in `buffered`,
/// reading what the socket offers right now into it.
fn poll_frame(
    reader: &mut BufReader<TcpStream>,
    buffered: &mut Vec<u8>,
) -> Result<Option<Vec<u8>>, AttemptError> {
    use std::io::Read as _;
    let mut chunk = [0_u8; 4096];
    match reader.read(&mut chunk) {
        Ok(0) => return Err(AttemptError::Transient("connection closed".to_owned())),
        Ok(read) => buffered.extend_from_slice(&chunk[..read]),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
            ) => {}
        Err(error) => return Err(AttemptError::Transient(format!("read: {error}"))),
    }
    if buffered.len() < 2 {
        return Ok(None);
    }
    if buffered[0] != 0x81 {
        return Err(AttemptError::Fatal(format!(
            "expected a final text frame, got {:#x}",
            buffered[0]
        )));
    }
    let mut size = u64::from(buffered[1] & 0x7f);
    let mut header_len = 2;
    if size == 126 {
        if buffered.len() < 4 {
            return Ok(None);
        }
        size = u64::from(u16::from_be_bytes([buffered[2], buffered[3]]));
        header_len = 4;
    } else if size == 127 {
        if buffered.len() < 10 {
            return Ok(None);
        }
        size = u64::from_be_bytes(buffered[2..10].try_into().unwrap());
        header_len = 10;
    }
    let total = header_len + usize::try_from(size).map_err(transient)?;
    if buffered.len() < total {
        return Ok(None);
    }
    Ok(Some(buffered.drain(..total).skip(header_len).collect()))
}
