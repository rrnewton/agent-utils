//! HTTPS GET through the system `curl`, with secret headers kept off the command line.
//!
//! Shelling out keeps the binary free of a TLS stack and inherits the host's proxy environment
//! (`https_proxy`); `-q` makes curl ignore any `.curlrc`. Headers are written to curl's stdin as a `--config -`
//! file, so a bearer token never appears in `ps` output or `/proc/<pid>/cmdline`.

use std::io::Write;
use std::process::{Command, Stdio};

/// A completed request: HTTP status (0 for `file://` URLs) and the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP status code; `0` when the scheme has none (`file://`, used by tests).
    pub status: u16,
    /// Response body as text.
    pub body: String,
}

fn config_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => {}
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The `--config -` text for these headers.
pub fn header_config(headers: &[(&str, &str)]) -> String {
    let mut text = String::new();
    for (name, value) in headers {
        text.push_str("header = ");
        text.push_str(&config_quote(&format!("{name}: {value}")));
        text.push('\n');
    }
    text
}

/// GET `url` with `headers`, giving up after `timeout_secs`. `curl` is looked up on `PATH`
/// unless `AGENT_USAGE_CURL` names it.
pub fn get(url: &str, headers: &[(&str, &str)], timeout_secs: u64) -> Result<Response, String> {
    let curl = std::env::var("AGENT_USAGE_CURL").unwrap_or_else(|_| "curl".to_string());
    let mut child = Command::new(&curl)
        .args([
            "-q",
            "--silent",
            "--show-error",
            "--max-time",
            &timeout_secs.to_string(),
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
            "--url",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {curl}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(header_config(headers).as_bytes())
            .map_err(|e| format!("cannot write curl config: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("curl failed: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "curl exit {}: {}",
            out.status.code().unwrap_or(-1),
            err.trim().chars().take(200).collect::<String>()
        ));
    }
    let (body, code) = match text.rfind('\n') {
        Some(at) => (text[..at].to_string(), text[at + 1..].trim().to_string()),
        None => (String::new(), text.trim().to_string()),
    };
    let status = code.parse::<u16>().unwrap_or(0);
    Ok(Response { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_quotes_and_strips_newlines() {
        let text = header_config(&[("A", "b\"c\\d\ne")]);
        assert_eq!(text, "header = \"A: b\\\"c\\\\de\"\n");
    }

    #[test]
    fn reads_a_file_url() {
        let dir = std::env::temp_dir().join(format!("agent-usage-http-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("body.json");
        std::fs::write(&file, "{\"ok\":true}").unwrap();
        let resp = get(&format!("file://{}", file.display()), &[("X", "y")], 5).unwrap();
        assert_eq!(resp.status, 0);
        assert_eq!(resp.body, "{\"ok\":true}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
