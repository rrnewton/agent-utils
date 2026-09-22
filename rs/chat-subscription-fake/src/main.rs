//! Standalone conformance plugin used by process-protocol tests and host smoke checks.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::time::Duration;

use chat_subscription_fake::FakeBackend;

fn main() {
    if let Ok(mode) = std::env::var("CHAT_SUBSCRIPTION_FAKE_STALL") {
        if let Err(error) = run_stalled_fixture(&mode) {
            eprintln!("stalled fake chat plugin failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    let mut backend = FakeBackend::conformance("fake-process-plugin");
    if let Err(error) = chat_subscription_plugin::serve_stdio(&mut backend) {
        eprintln!("fake chat plugin failed: {error}");
        std::process::exit(1);
    }
}

fn read_frame(reader: &mut impl Read) -> io::Result<serde_json::Value> {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > chat_subscription_plugin::MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "fixture received an invalid frame length",
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(io::Error::other)
}

fn write_frame(writer: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    let length = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "fixture frame is too large"))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

fn expect_type(value: &serde_json::Value, expected: &str) -> io::Result<()> {
    if value.get("type").and_then(serde_json::Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("fixture expected {expected}, found {value}"),
        ))
    }
}

fn hello(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    expect_type(&read_frame(reader)?, "hello")?;
    write_frame(
        writer,
        &serde_json::json!({
            "type": "hello",
            "version": 1,
            "capabilities": {
                "backend_name": "stall-fixture",
                "replay": "cursor",
                "full_message_data": false,
                "max_uncommitted": 1,
                "event_kinds": ["checkpoint", "heartbeat"]
            }
        }),
    )
}

fn start(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    expect_type(&read_frame(reader)?, "start")?;
    write_frame(writer, &serde_json::json!({"type": "subscribed"}))
}

fn stall() {
    std::thread::sleep(Duration::from_secs(60));
}

fn run_stalled_fixture(mode: &str) -> io::Result<()> {
    let input = io::stdin();
    let output = io::stdout();
    let mut reader = input.lock();
    let mut writer = output.lock();
    match mode {
        "hello" => stall(),
        "start" => {
            hello(&mut reader, &mut writer)?;
            stall();
        }
        "commit" => {
            hello(&mut reader, &mut writer)?;
            start(&mut reader, &mut writer)?;
            write_frame(
                &mut writer,
                &serde_json::json!({
                    "type": "item",
                    "item": {
                        "kind": "batch",
                        "sequence": 1,
                        "provider_cursor": "cursor-one",
                        "delivery_id": "delivery-one",
                        "events": [{"kind": "checkpoint"}]
                    }
                }),
            )?;
            expect_type(&read_frame(&mut reader)?, "commit")?;
            stall();
        }
        "end-exit-7" => {
            hello(&mut reader, &mut writer)?;
            start(&mut reader, &mut writer)?;
            write_frame(&mut writer, &serde_json::json!({"type": "end"}))?;
            std::process::exit(7);
        }
        "end-stall" => {
            hello(&mut reader, &mut writer)?;
            start(&mut reader, &mut writer)?;
            write_frame(&mut writer, &serde_json::json!({"type": "end"}))?;
            stall();
        }
        "close-before-start" => {
            hello(&mut reader, &mut writer)?;
            expect_type(&read_frame(&mut reader)?, "close")?;
            stall();
        }
        "close" => {
            hello(&mut reader, &mut writer)?;
            start(&mut reader, &mut writer)?;
            expect_type(&read_frame(&mut reader)?, "close")?;
            stall();
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown stall mode {other}"),
            ));
        }
    }
    Ok(())
}
