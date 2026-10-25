//! Routes: how the sender reaches a contact's host (design section 10).
//! Drivers are in code behind the [`Route`] trait; routes are data.
//! Every host route ends at the receiving host's own `agent-teams serve`.

use crate::jsonfile::DataResult;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// How one attempt over a host route ended (design section 10.2).
#[derive(Debug, Clone)]
pub enum AttemptOutcome {
    /// The remote agent-teams answered the protocol; the JSON decides.
    Answered(Value),
    /// Definite: the route proves the envelope cannot have reached `accept`.
    NotDelivered(String),
    /// Anything else; resolved by asking, never by resending blind.
    Ambiguous(String),
}

/// A route: a way to run `agent-teams serve VERB` on a target host.
pub trait Route {
    /// Route name from `routes.d`.
    fn name(&self) -> &str;
    /// Run one verb (`accept`, `receipt`, `fence`, `probe`, `ping`) with a
    /// JSON payload, classifying the outcome.
    fn run_verb(&self, verb: &str, payload: &Value) -> AttemptOutcome;
    /// Whether bodies on this route are visible beyond the two agent-teams
    /// processes (chat routes, hub relay); `--private` skips such routes.
    fn exposes_body(&self) -> bool {
        false
    }
    /// A definite skip reason known before attempting (link down, too large).
    fn skip_reason(&self, _envelope_bytes: usize) -> Option<String> {
        None
    }
}

/// The serve banner. A driver writes the envelope only after reading it, so
/// no banner means the envelope was never sent: definite (section 10.2).
pub fn banner(host: &str) -> String {
    format!(
        "agent-teams-serve/1 ready {} {}",
        host,
        crate::timeutil::boot_id()
    )
}

/// Read the banner line from a serve child; `None` ends the attempt as
/// not-delivered.
fn read_banner(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(n) if n > 0 && line.starts_with("agent-teams-serve/1 ready ") => {
            Some(line.trim().to_string())
        }
        _ => None,
    }
}

/// Write one length-prefixed frame: `<verb>\n<len>\n<payload>`.
fn write_frame(writer: &mut impl Write, verb: &str, payload: &Value) -> std::io::Result<()> {
    let text = payload.to_string();
    writeln!(writer, "{verb}")?;
    writeln!(writer, "{}", text.len())?;
    writer.write_all(text.as_bytes())?;
    writer.flush()
}

/// Read one frame (used by `serve`). Acts only on a complete frame whose
/// JSON parses, so a cut-off write does nothing.
pub fn read_frame(reader: &mut impl BufRead) -> Option<(String, Value)> {
    let mut verb = String::new();
    if reader.read_line(&mut verb).ok()? == 0 {
        return None;
    }
    let mut len_line = String::new();
    reader.read_line(&mut len_line).ok()?;
    let len: usize = len_line.trim().parse().ok()?;
    if len > 16 * 1024 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    let payload: Value = serde_json::from_slice(&buf).ok()?;
    Some((verb.trim().to_string(), payload))
}

fn run_child_verb(mut child: Child, verb: &str, payload: &Value) -> AttemptOutcome {
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return AttemptOutcome::NotDelivered("serve child has no stdout".to_string());
    };
    let mut reader = BufReader::new(stdout);
    if read_banner(&mut reader).is_none() {
        let _ = child.kill();
        let _ = child.wait();
        return AttemptOutcome::NotDelivered("no serve banner; envelope never sent".to_string());
    }
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        return AttemptOutcome::Ambiguous("serve child has no stdin".to_string());
    };
    if write_frame(&mut stdin, verb, payload).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return AttemptOutcome::Ambiguous("frame write failed after the banner".to_string());
    }
    drop(stdin);
    let mut answer = String::new();
    if reader.read_line(&mut answer).unwrap_or(0) == 0 {
        let _ = child.wait();
        return AttemptOutcome::Ambiguous("no answer after the envelope was sent".to_string());
    }
    let _ = child.wait();
    match serde_json::from_str::<Value>(answer.trim()) {
        Ok(value) => AttemptOutcome::Answered(value),
        Err(e) => AttemptOutcome::Ambiguous(format!("unreadable answer: {e}")),
    }
}

/// The built-in `local` route: `serve` as a child of this binary.
pub struct LocalRoute {
    /// Route name.
    pub name: String,
    /// Binary to run (this executable).
    pub program: PathBuf,
    /// Extra arguments (`--state-dir` is not a flag; env is inherited).
    pub peer: String,
}

impl Route for LocalRoute {
    fn name(&self) -> &str {
        &self.name
    }
    fn run_verb(&self, verb: &str, payload: &Value) -> AttemptOutcome {
        let child = Command::new(&self.program)
            .args(["serve", "--stdio", "--peer", &self.peer])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(child) => run_child_verb(child, verb, payload),
            Err(e) => AttemptOutcome::NotDelivered(format!("local serve failed to start: {e}")),
        }
    }
}

/// The `ssh` driver (design section 10.3). With `use_user_ssh_config` it runs
/// OpenSSH as the owner does; otherwise it runs with `-F none`, a dedicated
/// identity and its own known-hosts file.
pub struct SshRoute {
    /// Route name.
    pub name: String,
    /// ssh target host.
    pub target: String,
    /// ssh user, when set.
    pub user: Option<String>,
    /// Dedicated identity file, when set.
    pub identity: Option<PathBuf>,
    /// Own known-hosts file, when set.
    pub known_hosts: Option<PathBuf>,
    /// Use the owner's ssh configuration (allowed permanently, section 10.3).
    pub use_user_ssh_config: bool,
    /// Remote `agent-teams` binary path.
    pub remote_binary: String,
    /// This host's key, passed as `--peer`.
    pub peer: String,
    /// Connect timeout in seconds.
    pub connect_timeout: u32,
    /// Another route's control socket to reuse (managed link), when up.
    pub control_path: Option<PathBuf>,
    /// Link mode: `none`, `on-demand` or `persistent`.
    pub link_mode: String,
    /// Link health file (for the known-down skip).
    pub link_state: Option<PathBuf>,
    /// Maximum envelope bytes this route accepts.
    pub max_envelope_bytes: Option<usize>,
}

impl SshRoute {
    /// Build from route data, applying defaults.
    pub fn from_def(
        name: &str,
        raw: &Value,
        peer: &str,
        remote_binary: &str,
        links_dir: &std::path::Path,
    ) -> SshRoute {
        let expand = |key: &str| {
            raw.get(key)
                .and_then(Value::as_str)
                .map(crate::jsonfile::expand_home)
        };
        let link_mode = raw
            .get("link")
            .and_then(|l| l.get("mode"))
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string();
        let control_path = if link_mode == "none" {
            None
        } else {
            Some(control_socket(name))
        };
        SshRoute {
            name: name.to_string(),
            target: raw
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            user: raw.get("user").and_then(Value::as_str).map(str::to_string),
            identity: expand("identity"),
            known_hosts: expand("known_hosts"),
            use_user_ssh_config: raw
                .get("use_user_ssh_config")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            remote_binary: remote_binary.to_string(),
            peer: peer.to_string(),
            connect_timeout: raw
                .get("connect_timeout_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(10) as u32,
            control_path,
            link_mode,
            link_state: Some(links_dir.join(format!("{name}.json"))),
            max_envelope_bytes: raw
                .get("capabilities")
                .and_then(|c| c.get("max_envelope_bytes"))
                .and_then(Value::as_u64)
                .map(|n| n as usize),
        }
    }

    /// The ssh argv for one session (without the verb frame).
    pub fn ssh_argv(&self) -> Vec<String> {
        let mut argv: Vec<String> = vec!["ssh".to_string()];
        if !self.use_user_ssh_config {
            argv.extend(["-F".into(), "none".into()]);
            argv.extend(["-o".into(), "BatchMode=yes".into()]);
            argv.extend(["-o".into(), "IdentitiesOnly=yes".into()]);
            argv.extend(["-o".into(), "IdentityAgent=none".into()]);
            argv.extend(["-o".into(), "ForwardAgent=no".into()]);
            argv.extend(["-o".into(), "ClearAllForwardings=yes".into()]);
            argv.extend(["-o".into(), "StrictHostKeyChecking=yes".into()]);
            if let Some(identity) = &self.identity {
                argv.extend(["-i".into(), identity.to_string_lossy().into_owned()]);
            }
            if let Some(known_hosts) = &self.known_hosts {
                argv.extend([
                    "-o".into(),
                    format!("UserKnownHostsFile={}", known_hosts.display()),
                ]);
            }
        } else {
            argv.extend(["-o".into(), "BatchMode=yes".into()]);
        }
        argv.extend([
            "-o".into(),
            format!("ConnectTimeout={}", self.connect_timeout),
        ]);
        argv.extend(["-o".into(), "ServerAliveInterval=15".into()]);
        argv.extend(["-o".into(), "ServerAliveCountMax=3".into()]);
        if let Some(control) = &self.control_path {
            argv.extend(["-o".into(), format!("ControlPath={}", control.display())]);
            if self.link_mode == "on-demand" {
                argv.extend(["-o".into(), "ControlMaster=auto".into()]);
                argv.extend(["-o".into(), "ControlPersist=120".into()]);
            }
        }
        if let Some(user) = &self.user {
            argv.extend(["-l".into(), user.clone()]);
        }
        argv.push(self.target.clone());
        argv.extend([
            self.remote_binary.clone(),
            "serve".into(),
            "--stdio".into(),
            "--peer".into(),
            self.peer.clone(),
        ]);
        argv
    }
}

impl Route for SshRoute {
    fn name(&self) -> &str {
        &self.name
    }
    fn skip_reason(&self, envelope_bytes: usize) -> Option<String> {
        if let Some(max) = self.max_envelope_bytes {
            if envelope_bytes > max {
                return Some(format!(
                    "envelope of {envelope_bytes} bytes over route limit {max}"
                ));
            }
        }
        if let Some(state_path) = &self.link_state {
            if let Ok(text) = std::fs::read_to_string(state_path) {
                if let Ok(state) = serde_json::from_str::<Value>(&text) {
                    if state.get("state").and_then(Value::as_str) == Some("down") {
                        return Some("link known down".to_string());
                    }
                }
            }
        }
        None
    }
    fn run_verb(&self, verb: &str, payload: &Value) -> AttemptOutcome {
        let argv = self.ssh_argv();
        let child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        match child {
            Ok(child) => run_child_verb(child, verb, payload),
            Err(e) => AttemptOutcome::NotDelivered(format!("ssh failed to start: {e}")),
        }
    }
}

/// The control socket of a route's managed link (section 10.3).
pub fn control_socket(route: &str) -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime
        .join("agent-teams")
        .join("links")
        .join(format!("{route}.sock"))
}

/// The `command` driver in host mode (section 10.4): an argv template runs
/// `agent-teams serve` on the target host. Only declared `pre_exec_failure`
/// exits are definite; without a banner the attempt is ambiguous, because
/// the envelope was in the first write.
pub struct CommandRoute {
    /// Route name.
    pub name: String,
    /// Argv template; `{remote...}` splices the remote command.
    pub argv: Vec<String>,
    /// Template variables (`{vars.X}`, `{host.address}`).
    pub vars: std::collections::BTreeMap<String, String>,
    /// Remote `agent-teams` binary path.
    pub remote_binary: String,
    /// This host's key, passed as `--peer`.
    pub peer: String,
    /// Declared pre-exec failures: (exit code, stderr substring).
    pub pre_exec_failures: Vec<(i32, String)>,
    /// Whether the transport holds a conversation (banner protocol).
    pub interactive: bool,
}

impl CommandRoute {
    fn fill(&self, element: &str) -> String {
        let mut out = element.to_string();
        for (key, value) in &self.vars {
            out = out.replace(&format!("{{vars.{key}}}"), value);
            out = out.replace(&format!("{{{key}}}"), value);
        }
        out
    }
    fn full_argv(&self) -> Vec<String> {
        let remote = [
            self.remote_binary.clone(),
            "serve".to_string(),
            "--stdio".to_string(),
            "--peer".to_string(),
            self.peer.clone(),
        ];
        let mut argv = Vec::new();
        for element in &self.argv {
            if element == "{remote...}" {
                argv.extend(remote.iter().cloned());
            } else {
                argv.push(self.fill(element));
            }
        }
        argv
    }
}

impl Route for CommandRoute {
    fn name(&self) -> &str {
        &self.name
    }
    fn run_verb(&self, verb: &str, payload: &Value) -> AttemptOutcome {
        let argv = self.full_argv();
        let Some((program, args)) = argv.split_first() else {
            return AttemptOutcome::NotDelivered("command route has no argv".to_string());
        };
        let spawned = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                return AttemptOutcome::NotDelivered(format!("command route failed to start: {e}"))
            }
        };
        if !self.interactive {
            // The envelope goes in the first write, so any execution may have
            // accepted it; only declared pre-exec exits are definite.
            if let Some(mut stdin) = child.stdin.take() {
                let _ = writeln!(stdin, "{verb}");
                let _ = writeln!(stdin, "{}", payload.to_string().len());
                let _ = stdin.write_all(payload.to_string().as_bytes());
            }
            let output = child.wait_with_output();
            return match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if let Some(line) = stdout.lines().next_back() {
                        if let Ok(value) = serde_json::from_str::<Value>(line) {
                            return AttemptOutcome::Answered(value);
                        }
                    }
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let code = output.status.code().unwrap_or(-1);
                    if self
                        .pre_exec_failures
                        .iter()
                        .any(|(exit, needle)| *exit == code && stderr.contains(needle))
                    {
                        AttemptOutcome::NotDelivered(format!(
                            "declared pre-exec failure (exit {code})"
                        ))
                    } else {
                        AttemptOutcome::Ambiguous(format!(
                            "command route exit {code} without an answer"
                        ))
                    }
                }
                Err(e) => AttemptOutcome::Ambiguous(format!("command route failed: {e}")),
            };
        }
        run_child_verb(child, verb, payload)
    }
}

/// Build a route object from its definition.
pub fn route_for(
    def: &crate::model::RouteDef,
    this_host: &str,
    remote_binary: &str,
    links_dir: &std::path::Path,
) -> DataResult<Box<dyn Route>> {
    match def.driver.as_str() {
        "local" => Ok(Box::new(LocalRoute {
            name: def.name.clone(),
            program: std::env::current_exe()?,
            peer: this_host.to_string(),
        })),
        "ssh" => Ok(Box::new(SshRoute::from_def(
            &def.name,
            &def.raw,
            this_host,
            remote_binary,
            links_dir,
        ))),
        "command" => {
            let argv: Vec<String> = def
                .raw
                .get("argv")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let mut vars = std::collections::BTreeMap::new();
            if let Some(obj) = def.raw.get("vars").and_then(Value::as_object) {
                for (key, value) in obj {
                    if let Some(text) = value.as_str() {
                        vars.insert(key.clone(), text.to_string());
                    }
                }
            }
            let mut pre_exec_failures = Vec::new();
            if let Some(list) = def
                .raw
                .get("outcome")
                .and_then(|o| o.get("pre_exec_failure"))
                .and_then(Value::as_array)
            {
                for item in list {
                    let exit = item.get("exit").and_then(Value::as_i64).unwrap_or(0) as i32;
                    // The design's stderr_regex is matched as a substring here;
                    // routes should declare a literal prefix.
                    let needle = item
                        .get("stderr_regex")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim_start_matches('^')
                        .to_string();
                    pre_exec_failures.push((exit, needle));
                }
            }
            Ok(Box::new(CommandRoute {
                name: def.name.clone(),
                argv,
                vars,
                remote_binary: remote_binary.to_string(),
                peer: this_host.to_string(),
                pre_exec_failures,
                interactive: def
                    .raw
                    .get("interactive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }))
        }
        other => Err(crate::jsonfile::DataError::new(
            "route",
            format!("route {} has unknown driver {other:?}", def.name),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frame_round_trips() {
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, "ping", &json!({"x": 1})).expect("write");
        let mut reader = BufReader::new(buf.as_slice());
        let (verb, payload) = read_frame(&mut reader).expect("read");
        assert_eq!(verb, "ping");
        assert_eq!(payload["x"], 1);
    }

    #[test]
    fn cut_off_frame_is_no_frame() {
        let data = b"accept\n99\n{\"short\"";
        let mut reader = BufReader::new(data.as_slice());
        assert!(read_frame(&mut reader).is_none());
    }

    #[test]
    fn ssh_argv_without_user_config_is_restricted() {
        let raw = json!({"driver": "ssh", "target": "host-b.example.net", "user": "u"});
        let route = SshRoute::from_def(
            "ssh-b",
            &raw,
            "host-a",
            "~/bin/agent-teams",
            PathBuf::from("/tmp").as_path(),
        );
        let argv = route.ssh_argv();
        assert!(argv.windows(2).any(|w| w == ["-F", "none"]));
        assert!(argv.iter().any(|a| a == "BatchMode=yes"));
        assert!(argv.iter().any(|a| a == "host-b.example.net"));
    }
}
