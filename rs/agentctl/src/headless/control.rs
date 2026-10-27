//! The headless wrapper's control socket, `<registry>/<agent>/control.sock`.
//!
//! A Unix stream socket, mode 0600 in the owner-only agent directory, with no network listener.
//! Each connection carries one JSON request line and gets one JSON response line. Requests:
//! `{"op": "wake"}` (look at the queue now), `{"op": "status"}`, `{"op": "interrupt"}` (stop the
//! running turn), `{"op": "stop"}` (finish cleanly and exit). Every response has `ok`; a refused
//! request has `ok: false` and `error`.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// File name of the socket inside the agent directory.
pub const SOCKET: &str = "control.sock";
/// Longest request line accepted.
const MAX_REQUEST: u64 = 64 * 1024;
/// How long the server waits for the wrapper's answer before replying `busy`.
const ANSWER_WAIT: Duration = Duration::from_secs(15);

/// One request handed to the wrapper's main loop, with the channel for its answer.
#[derive(Debug)]
pub struct Request {
    /// The parsed request object.
    pub body: Value,
    /// Where the main loop sends the response.
    pub reply: SyncSender<Value>,
}

/// The socket path of the agent whose directory is `agent_directory`.
#[must_use]
pub fn socket_path(agent_directory: &Path) -> PathBuf {
    agent_directory.join(SOCKET)
}

/// Bind the socket (replacing a stale one; the caller holds the headless lock) and serve it on a
/// background thread, handing each request to `sink`. Returns the bound path.
pub fn serve<T: Send + 'static>(
    path: &Path,
    sink: Sender<T>,
    wrap: fn(Request) -> T,
) -> io::Result<PathBuf> {
    if path.as_os_str().len() >= 108 {
        return Err(io::Error::other(format!(
            "control socket path is too long for a Unix socket ({} bytes): {}",
            path.as_os_str().len(),
            path.display()
        )));
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    thread::Builder::new()
        .name("headless-control".to_owned())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let sink = sink.clone();
                let _ = thread::Builder::new()
                    .name("headless-control-conn".to_owned())
                    .spawn(move || {
                        let _ = handle(stream, &sink, wrap);
                    });
            }
        })?;
    Ok(path.to_path_buf())
}

fn handle<T>(stream: UnixStream, sink: &Sender<T>, wrap: fn(Request) -> T) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    {
        let mut reader = BufReader::new(io::Read::take(&stream, MAX_REQUEST));
        reader.read_line(&mut line)?;
    }
    let response = match serde_json::from_str::<Value>(line.trim()) {
        Ok(body) if body.get("op").and_then(Value::as_str).is_some() => {
            let (reply, answer) = mpsc::sync_channel(1);
            if sink.send(wrap(Request { body, reply })).is_err() {
                json!({"ok": false, "error": "the wrapper is shutting down"})
            } else {
                answer.recv_timeout(ANSWER_WAIT).unwrap_or_else(
                    |_| json!({"ok": false, "error": "the wrapper did not answer in time"}),
                )
            }
        }
        _ => json!({"ok": false, "error": "a request is one JSON object with an \"op\" string"}),
    };
    let mut stream = stream;
    let mut bytes = serde_json::to_vec(&response)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)
}

/// Why a control request got no answer.
#[derive(Debug)]
pub enum ClientError {
    /// No wrapper listens: the socket is absent or refuses connections.
    NotRunning(String),
    /// A wrapper is there but did not answer properly in time.
    Unreachable(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning(detail) => {
                write!(formatter, "the headless wrapper is not running ({detail})")
            }
            Self::Unreachable(detail) => {
                write!(formatter, "the headless wrapper did not answer ({detail})")
            }
        }
    }
}

/// Send one request to the wrapper at `path` and return its response.
pub fn request(path: &Path, body: &Value, timeout: Duration) -> Result<Value, ClientError> {
    let mut stream = UnixStream::connect(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
            ClientError::NotRunning(error.to_string())
        }
        _ => ClientError::Unreachable(error.to_string()),
    })?;
    let unreachable = |error: io::Error| ClientError::Unreachable(error.to_string());
    stream
        .set_read_timeout(Some(timeout))
        .map_err(unreachable)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(unreachable)?;
    let mut bytes =
        serde_json::to_vec(body).map_err(|error| ClientError::Unreachable(error.to_string()))?;
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(unreachable)?;
    let mut line = String::new();
    BufReader::new(io::Read::take(&stream, MAX_REQUEST))
        .read_line(&mut line)
        .map_err(unreachable)?;
    serde_json::from_str(line.trim())
        .map_err(|error| ClientError::Unreachable(format!("unreadable answer: {error}")))
}

/// A receiver of requests for tests and simple callers.
pub fn channel() -> (Sender<Request>, Receiver<Request>) {
    mpsc::channel()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_and_absent_socket_is_not_running() {
        let dir = std::env::temp_dir().join(format!("agentctl-control-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = socket_path(&dir);
        assert!(matches!(
            request(&path, &json!({"op": "status"}), Duration::from_secs(1)),
            Err(ClientError::NotRunning(_))
        ));
        let (sender, receiver) = channel();
        serve(&path, sender, |request| request).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let server = thread::spawn(move || {
            let request = receiver.recv().unwrap();
            assert_eq!(request.body["op"], "status");
            request
                .reply
                .send(json!({"ok": true, "status": "idle"}))
                .unwrap();
        });
        let answer = request(&path, &json!({"op": "status"}), Duration::from_secs(5)).unwrap();
        assert_eq!(answer, json!({"ok": true, "status": "idle"}));
        server.join().unwrap();
        let answer = {
            let mut stream = UnixStream::connect(&path).unwrap();
            stream.write_all(b"not json\n").unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        };
        assert_eq!(answer["ok"], false);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
