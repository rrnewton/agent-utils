//! The startup channel probe, exercised through the actual binary.
//!
//! The unit tests in `src/probe.rs` prove the probe classifies correctly. They cannot prove
//! anybody calls it — a probe that exists and is never invoked is the exact silent no-op this
//! feature was added to remove, and it would leave every unit test green. So these tests run the
//! real `gent-talk` binary, against a Discord that is not there, and check what it does.

use std::io::{BufRead as _, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// A configuration whose Discord is a closed port, so every read fails at the transport.
///
/// Port 1 is reserved and nothing listens on it, which makes "the request never reached Discord"
/// the deterministic answer rather than a timing accident.
fn config_text() -> String {
    r#"
[server]
bind = "127.0.0.1:0"

[discord]
bot_token = "startup-probe-test-bot-token"
api_base = "http://127.0.0.1:1"

[auth]
read_token = "startup-read-token-000000000000"
write_token = "startup-write-token-00000000000"

[[channels]]
id = "1111111111"
label = "lead team"
writable = true

[[channels]]
id = "2222222222"
label = "build noise"
writable = false
"#
    .to_owned()
}

fn write_config(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gent-talk-startup-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("gent-talk.toml");
    std::fs::write(&path, config_text()).expect("write config");
    path
}

/// The binary under test, built by cargo for this integration test.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_gent-talk")
}

/// Wait up to `limit` for the child to exit, killing it if it does not.
fn wait_for_exit(
    child: &mut std::process::Child,
    limit: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait().expect("polling the child") {
            Some(status) => return Some(status),
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Everything the child printed, both streams together.
fn collect_output(child: std::process::Child) -> String {
    let out = child.wait_with_output().expect("collecting output");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn an_unreachable_channel_stops_the_server_from_starting() {
    let config = write_config("fails");
    let mut child = Command::new(binary())
        .arg("--config")
        .arg(&config)
        // Make sure an ambient value on the developer's machine cannot turn this check off and
        // silently pass the test.
        .env_remove("GENT_TALK_SKIP_STARTUP_PROBE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning the binary");

    // Bounded, because the interesting regression is a probe that passes when it should not — and
    // then this server does exactly what it is built to do and serves forever. Waiting on exit
    // without a deadline would turn that regression into a hung suite instead of a red one.
    let status = wait_for_exit(&mut child, Duration::from_secs(20));
    let output = collect_output(child);
    let status = status.unwrap_or_else(|| {
        panic!(
            "the server did not exit at all: it must REFUSE to start when a configured channel \
             cannot be read, and instead it kept running.\n{output}"
        )
    });

    assert!(
        !status.success(),
        "the server must REFUSE to start when a configured channel cannot be read; it exited \
         {status:?}\n{output}"
    );
    let text = output;
    assert!(
        text.contains("FAILED"),
        "no failure marker in output:\n{text}"
    );
    assert!(
        text.contains("1111111111") && text.contains("lead team"),
        "every failing channel must be named by id AND by the label the operator wrote:\n{text}"
    );
    assert!(
        text.contains("never reached Discord"),
        "the cause must be stated, not collapsed into 'unreachable':\n{text}"
    );
    assert!(
        text.contains("refusing to start"),
        "the refusal must say it is a refusal:\n{text}"
    );
}

#[test]
fn the_skip_switch_starts_the_server_anyway_and_says_so() {
    let config = write_config("skips");
    let mut child = Command::new(binary())
        .arg("--config")
        .arg(&config)
        .env("GENT_TALK_SKIP_STARTUP_PROBE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning the binary");

    // Read both streams: the structured log goes to stdout, the operator-facing warning to stderr.
    let (tx, rx) = mpsc::channel::<String>();
    for stream in [
        Box::new(child.stdout.take().expect("stdout")) as Box<dyn std::io::Read + Send>,
        Box::new(child.stderr.take().expect("stderr")),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
    }
    drop(tx);

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut seen = String::new();
    let mut warned = false;
    let mut listening = false;
    while std::time::Instant::now() < deadline && !(warned && listening) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                if line.contains("SKIPPING the startup channel probe")
                    || line.contains("startup channel probe SKIPPED")
                {
                    warned = true;
                }
                if line.contains("gent-talk listening") {
                    listening = true;
                }
                seen.push_str(&line);
                seen.push('\n');
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        listening,
        "with the probe skipped the server must start despite an unreachable Discord:\n{seen}"
    );
    assert!(
        warned,
        "a skipped probe must announce itself; an invisible skip is the silent start this \
         feature exists to prevent:\n{seen}"
    );
}
