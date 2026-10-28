//! The approval surface of a headless agent (`#671 headless-approval-surface`).
//!
//! A headless harness has no terminal on which a person could answer a permission prompt, so
//! the wrapper answers them. It runs Claude Code with `--permission-prompts host
//! --permission-prompt-tool stdio` ([`super::claude::effective_arguments`]); each prompt the
//! harness raises (a tool call that needs approval: a hook's or classifier's `ask`, a path
//! outside the allowed directories) is held while the owner is asked:
//!
//! 1. The wrapper logs `approval.requested` with a short ID (`a<seq>`), the tool, the masked
//!    command summary, the whole call as the owner should judge it (`detail`, with `complete`
//!    false when it had to be cut or masked) and the reason it asked, and starts the timeout.
//! 2. A chat bridge serving the agent (`agentctl chat run`) follows the event log and, once the
//!    prompt has waited a few seconds (the host's own permission hooks settle many prompts
//!    first), posts "AGENT wants to run: TOOL (reason: ...) - reply `approve ID` or `deny ID`"
//!    with the call beneath it to its channel ([`request_text`]); `agentctl tail` shows the
//!    same event.
//! 3. The owner answers. The bridge admits only its verified owners' messages; one that reads
//!    as an answer naming a prompt ([`parse_reply`]; in chat the ID is required) is passed to the
//!    wrapper as a control request `{"op": "approval", "id", "decision", "by": "chat:<sender>",
//!    "queue_id"}` before its prompt is queued, and the wrapper then takes that queued prompt as
//!    consumed instead of typing it. An answer the wrapper does not take is logged as
//!    `approval.refused`, which the bridge posts back. The owner at a terminal can answer with
//!    `agentctl approve` or `agentctl deny`. A chat approval of a call whose detail is not
//!    complete is refused: only the console can approve what the owner could not read.
//! 4. The wrapper answers the harness: allow (the call runs unchanged) or deny (the model reads
//!    the reason as the tool's error), and logs `approval.decided`. No answer within the timeout
//!    denies the call; a harness that withdraws the prompt, ends the turn or exits ends it as
//!    `cancelled`. The wrapper never approves anything by itself (the host's own permission
//!    hooks may, before the wrapper is asked, as they would in an interactive session).
//!
//! A hook's outright refusal never prompts, so it stays a refusal. The wrapper refuses an answer
//! from the agent's own processes ([`peer_refusal`]): one whose control-socket peer descends
//! from the harness, shares the harness's control group (a process that left the harness's
//! process tree with `setsid` stays in its systemd unit), or runs, itself or through an
//! ancestor, with `AGENTCTL_AGENT` naming the agent. This stops the agent approving its own
//! request by accident or by following posted instructions; it is not a boundary against a
//! hostile process of the same user, which could write any file the wrapper reads or post to
//! the chat with the owner's own credentials.

use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::Value;

use super::events::excerpt;

/// How long a prompt waits for the owner before it is denied, by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest reason quoted in a chat post, in bytes.
pub const CHAT_REASON_BYTES: usize = 600;
/// Longest chat message read as an answer, in bytes.
const MAX_REPLY_BYTES: usize = 500;

/// The owner's answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// Let the call run.
    Approve,
    /// Refuse it.
    Deny,
}

impl Decision {
    /// `approve` or `deny`, as the control request spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
        }
    }

    /// The control request's spelling, read back.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "approve" => Some(Self::Approve),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// A chat message read as an answer to an approval prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reply {
    /// Approve or deny.
    pub decision: Decision,
    /// The prompt it names (`a<digits>`); `None` answers the only one pending.
    pub id: Option<String>,
    /// The owner's reason for a denial, passed to the model.
    pub reason: Option<String>,
}

/// The ID of the prompt logged at event `seq`.
#[must_use]
pub fn approval_id(seq: u64) -> String {
    format!("a{seq}")
}

/// Whether `text` is an approval ID: `a` and one or more digits.
#[must_use]
pub fn valid_id(text: &str) -> bool {
    text.strip_prefix('a')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Read a chat message as an answer: `approve [ID]` or `allow [ID]`, nothing more; `deny [ID]
/// [reason]` or `reject [ID] [reason]`. Case, surrounding Markdown (backticks, stars,
/// underscores), a leading mention (`<@U123>`), a `#` before the ID and trailing punctuation are
/// ignored. Anything else, or a message longer than 500 bytes, is not an answer and is delivered
/// to the agent as usual.
#[must_use]
pub fn parse_reply(text: &str) -> Option<Reply> {
    if text.len() > MAX_REPLY_BYTES {
        return None;
    }
    let mut rest = text.trim();
    if rest.starts_with("<@") {
        rest = rest
            .split_once('>')
            .map_or("", |(_, after)| after)
            .trim_start();
    }
    let rest = rest.trim_matches(|c: char| matches!(c, '`' | '*' | '_' | '~') || c.is_whitespace());
    let (word, after) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(word, after)| (word, after.trim()));
    let word = word
        .trim_end_matches(['.', '!', ',', ':'])
        .to_ascii_lowercase();
    let decision = match word.as_str() {
        "approve" | "allow" => Decision::Approve,
        "deny" | "reject" => Decision::Deny,
        _ => return None,
    };
    let (first, remainder) = after
        .split_once(char::is_whitespace)
        .map_or((after, ""), |(first, remainder)| (first, remainder.trim()));
    let candidate = first
        .trim_start_matches('#')
        .trim_end_matches(['.', '!', ',', ':'])
        .to_ascii_lowercase();
    let (id, remainder) = if valid_id(&candidate) {
        (Some(candidate), remainder)
    } else {
        (None, after)
    };
    let remainder = remainder
        .trim_start_matches([':', '-', '—'])
        .trim()
        .trim_end_matches(['.', '!']);
    match decision {
        Decision::Approve if !remainder.is_empty() => None,
        Decision::Approve => Some(Reply {
            decision,
            id,
            reason: None,
        }),
        Decision::Deny => Some(Reply {
            decision,
            id,
            reason: (!remainder.is_empty()).then(|| remainder.to_owned()),
        }),
    }
}

/// Whether `by` names an approver the wrapper accepts: `chat:<sender>` (a bridge's verified
/// owner) or `console:<user>` (the owner at a terminal), one token of at most 200 bytes.
#[must_use]
pub fn valid_by(by: &str) -> bool {
    let rest = by
        .strip_prefix("chat:")
        .or_else(|| by.strip_prefix("console:"));
    by.len() <= 200
        && rest.is_some_and(|rest| {
            !rest.is_empty() && !rest.chars().any(|c| c.is_whitespace() || c.is_control())
        })
}

/// The process ID of the peer of a connected Unix socket, from `SO_PEERCRED`.
#[must_use]
pub fn peer_pid(stream: &UnixStream) -> Option<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `length` bytes into `credentials`.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(credentials).cast(),
            &mut length,
        )
    };
    (result == 0 && credentials.pid > 0).then(|| credentials.pid.unsigned_abs())
}

/// The parent of process `pid`, from `/proc/PID/stat`.
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is parenthesised and may hold spaces; the fields after it are plain.
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(1)?.parse().ok()
}

/// Whether `pid` is `ancestor` or one of its descendants.
#[must_use]
pub fn descends_from(pid: u32, ancestor: u32) -> bool {
    let mut current = pid;
    for _ in 0..128 {
        if current == ancestor {
            return true;
        }
        match parent_of(current) {
            Some(parent) if parent > 1 && parent != current => current = parent,
            _ => return false,
        }
    }
    false
}

/// Whether process `pid` runs with `AGENTCTL_AGENT=agent` in its environment.
#[must_use]
pub fn runs_as_agent(pid: u32, agent: &str) -> bool {
    let Ok(environment) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let wanted = format!("AGENTCTL_AGENT={agent}");
    environment
        .split(|byte| *byte == 0)
        .any(|entry| entry == wanted.as_bytes())
}

/// The first of `pid` and its ancestors that runs as `agent` ([`runs_as_agent`]), if any.
fn agent_in_ancestry(pid: u32, agent: &str) -> Option<u32> {
    let mut current = pid;
    for _ in 0..128 {
        if runs_as_agent(current, agent) {
            return Some(current);
        }
        match parent_of(current) {
            Some(parent) if parent > 1 && parent != current => current = parent,
            _ => return None,
        }
    }
    None
}

/// The control group of process `pid`, from `/proc/PID/cgroup`.
#[must_use]
pub fn cgroup_of(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .ok()
        .filter(|text| !text.trim().is_empty())
}

/// Why an answer from control-socket peer `peer` must be refused, if it must: the peer is
/// unknown, descends from the harness `harness`, shares its control group (with `cgroups`), or
/// it or an ancestor runs as the agent (a child that cleared its own environment is still
/// caught by its parent's).
#[must_use]
pub fn peer_refusal(
    peer: Option<u32>,
    harness: Option<u32>,
    agent: &str,
    cgroups: bool,
) -> Option<String> {
    let Some(peer) = peer else {
        return Some("the wrapper could not identify the process that sent the answer".to_owned());
    };
    if harness.is_some_and(|harness| descends_from(peer, harness)) {
        return Some(format!(
            "process {peer} runs inside the agent's own harness; an agent cannot answer its own approval prompt"
        ));
    }
    if let (true, Some(harness)) = (cgroups, harness) {
        if let (Some(theirs), Some(ours)) = (cgroup_of(peer), cgroup_of(harness)) {
            if theirs == ours {
                return Some(format!(
                    "process {peer} runs in the agent's own control group ({}); an agent cannot answer its own approval prompt (run the chat bridge and the wrapper as separate units)",
                    ours.lines().last().unwrap_or("").trim()
                ));
            }
        }
    }
    if let Some(process) = agent_in_ancestry(peer, agent) {
        return Some(format!(
            "process {peer} runs as agent {agent} (AGENTCTL_AGENT in process {process}); an agent cannot answer its own approval prompt"
        ));
    }
    None
}

fn text_of<'a>(data: &'a Value, key: &str) -> &'a str {
    data.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Text that is not ours (a reason, a description the model wrote) made inert in a chat post:
/// whitespace runs become one space, and the characters of Slack's markup and mentions (`<`,
/// `>`, `@`, `*`, `_`, `~`, backtick) are replaced by look-alikes that markup ignores.
fn chat_safe(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(|c| match c {
            '<' => '‹',
            '>' => '›',
            '@' => '＠',
            '*' => '∗',
            '_' => '＿',
            '~' => '∼',
            '`' => '\'',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

/// `text` safe inside a fenced code block: a run of three backticks is broken up.
fn fence_safe(text: &str) -> String {
    text.replace("```", "`\u{200b}``")
}

/// `text` safe inside a Markdown code span: backticks become quotes, line breaks spaces.
fn code_safe(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '`' => '\'',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

fn minutes(seconds: u64) -> String {
    if seconds % 60 == 0 && seconds >= 60 {
        format!("{} min", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

/// The chat post for an `approval.requested` event of `agent`: the question on one line, then
/// the whole call in a code block, and, when the call could not be shown whole, that only the
/// console can approve it. A reason the harness had to make up from the model's own description
/// of the call (`reason_type` `rule`) is labelled as the agent's words.
#[must_use]
pub fn request_text(agent: &str, data: &Value) -> String {
    let id = text_of(data, "id");
    let mut text = format!("{agent} wants to run: {}", text_of(data, "tool"));
    let reason = text_of(data, "reason").trim();
    if !reason.is_empty() {
        let label = if text_of(data, "reason_type") == "rule" {
            "the agent says"
        } else {
            "reason"
        };
        text.push_str(&format!(
            " ({label}: {})",
            excerpt(&chat_safe(reason), CHAT_REASON_BYTES)
        ));
    }
    if let Some(path) = data.get("blocked_path").and_then(Value::as_str) {
        text.push_str(&format!(
            " [outside the allowed directories: `{}`]",
            code_safe(path)
        ));
    }
    text.push_str(&format!(
        " - reply `approve {id}` or `deny {id}`; no answer in {} denies it.",
        minutes(data["timeout_s"].as_u64().unwrap_or(0))
    ));
    let detail = match text_of(data, "detail") {
        "" => text_of(data, "summary"),
        detail => detail,
    };
    if !detail.is_empty() {
        text.push_str(&format!("\n```\n{}\n```", fence_safe(detail)));
    }
    if data["complete"] == false {
        text.push_str(&format!(
            "\nShown cut or with credential-like words masked, so it can only be approved at the console (`agentctl approve {agent} {id}`); `deny {id}` works here."
        ));
    }
    text
}

/// The chat post for an `approval.refused` event of `agent`: why an answer was not taken.
#[must_use]
pub fn refused_text(agent: &str, data: &Value) -> String {
    let answer = match (text_of(data, "decision"), text_of(data, "id")) {
        ("", _) => "an approval answer".to_owned(),
        (decision, "") => format!("`{decision}`"),
        (decision, id) => format!("`{decision} {id}`"),
    };
    format!(
        "{agent}: {answer} was not taken: {}",
        excerpt(&chat_safe(text_of(data, "message")), CHAT_REASON_BYTES)
    )
}

/// The chat post for an `approval.decided` event of `agent`, or `None` for one not worth a post.
#[must_use]
pub fn decided_text(agent: &str, data: &Value) -> Option<String> {
    let id = text_of(data, "id");
    let what = match (text_of(data, "tool"), text_of(data, "summary")) {
        (tool, "") => tool.to_owned(),
        (tool, summary) => format!("{tool} `{}`", code_safe(summary)),
    };
    let by = text_of(data, "by");
    let who = if by.starts_with("chat:") {
        "in chat"
    } else if by.starts_with("console:") {
        "at the console"
    } else {
        ""
    };
    let outcome = match (text_of(data, "decision"), by) {
        ("approved", _) => format!("approved {who}"),
        ("denied", "timeout") => format!(
            "denied: no answer in {}",
            minutes(data["timeout_s"].as_u64().unwrap_or(0))
        ),
        ("denied", _) => format!("denied {who}"),
        ("cancelled", _) => "withdrawn: the harness no longer waits for it".to_owned(),
        _ => return None,
    };
    Some(format!("{agent}: {id} {} - {what}", outcome.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(decision: Decision, id: Option<&str>, reason: Option<&str>) -> Option<Reply> {
        Some(Reply {
            decision,
            id: id.map(str::to_owned),
            reason: reason.map(str::to_owned),
        })
    }

    #[test]
    fn answers_are_read_strictly() {
        assert_eq!(parse_reply("approve"), reply(Decision::Approve, None, None));
        assert_eq!(
            parse_reply(" Approve a12. "),
            reply(Decision::Approve, Some("a12"), None)
        );
        assert_eq!(
            parse_reply("`allow #a7`"),
            reply(Decision::Approve, Some("a7"), None)
        );
        assert_eq!(
            parse_reply("<@U1> approve a3"),
            reply(Decision::Approve, Some("a3"), None)
        );
        assert_eq!(parse_reply("deny"), reply(Decision::Deny, None, None));
        assert_eq!(
            parse_reply("deny a9: use the other remote"),
            reply(Decision::Deny, Some("a9"), Some("use the other remote"))
        );
        assert_eq!(
            parse_reply("reject too risky"),
            reply(Decision::Deny, None, Some("too risky"))
        );
        // An approval with anything after it is ordinary chat, never a blanket yes.
        assert_eq!(parse_reply("approve a12 and then push"), None);
        assert_eq!(parse_reply("approved the plan"), None);
        assert_eq!(parse_reply("yes"), None);
        assert_eq!(parse_reply("please approve a1"), None);
        assert_eq!(parse_reply(&format!("deny {}", "x".repeat(600))), None);
        assert!(valid_id("a1") && !valid_id("a") && !valid_id("b1") && !valid_id("a1x"));
    }

    #[test]
    fn approvers_must_be_chat_or_console_identities() {
        assert!(valid_by("chat:U0CA580U056"));
        assert!(valid_by("console:newton"));
        assert!(!valid_by("chat:"));
        assert!(!valid_by("agent:coord"));
        assert!(!valid_by("chat:a b"));
        assert!(!valid_by("timeout"));
    }

    #[test]
    fn the_agents_own_processes_cannot_answer() {
        let own = std::process::id();
        let parent = parent_of(own).expect("parent");
        assert!(descends_from(own, own));
        assert!(descends_from(own, parent));
        assert!(!descends_from(parent, own));
        assert!(peer_refusal(None, None, "a", false).is_some());
        assert!(peer_refusal(Some(own), Some(parent), "a", false).is_some());
        assert!(peer_refusal(Some(parent), Some(own), "no-such-agent-name", false).is_none());
        // A process outside the harness's tree but in its control group is refused too.
        assert_eq!(
            peer_refusal(Some(parent), Some(own), "no-such-agent-name", true).is_some(),
            cgroup_of(parent).is_some() && cgroup_of(parent) == cgroup_of(own)
        );
        let child = std::process::Command::new("sleep")
            .arg("5")
            .env("AGENTCTL_AGENT", "agent-under-test")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        // Until the child has run exec, /proc shows the parent's environment.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !runs_as_agent(pid, "agent-under-test") && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(runs_as_agent(pid, "agent-under-test"));
        assert!(!runs_as_agent(pid, "agent"));
        assert!(peer_refusal(Some(pid), None, "agent-under-test", false).is_some());
        // A child of such a process is caught by its parent's environment.
        assert_eq!(agent_in_ancestry(pid, "agent-under-test"), Some(pid));
        assert_eq!(agent_in_ancestry(own, "agent-under-test"), None);
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn chat_posts_name_the_command_reason_and_answers() {
        let data = json!({"id": "a42", "tool": "Bash", "summary": "git push origin",
                          "detail": "git push origin\n# ```done```",
                          "reason": "Refused by the\npermission hook: <!channel> *pushes* need a human",
                          "reason_type": "hook", "complete": true, "timeout_s": 600});
        assert_eq!(
            request_text("coord", &data),
            "coord wants to run: Bash (reason: Refused by the permission hook: ‹!channel› ∗pushes∗ need a human) - reply `approve a42` or `deny a42`; no answer in 10 min denies it.\n```\ngit push origin\n# `\u{200b}``done`\u{200b}``\n```"
        );
        // The model's own description is labelled as its words; a call not shown whole says
        // only the console can approve it.
        let data = json!({"id": "a43", "tool": "Bash", "detail": "rm x token=***",
                          "reason": "Pre-approved by the owner", "reason_type": "rule",
                          "complete": false, "timeout_s": 90});
        let text = request_text("coord", &data);
        assert!(
            text.starts_with(
                "coord wants to run: Bash (the agent says: Pre-approved by the owner)"
            ),
            "{text}"
        );
        assert!(text.contains("no answer in 90 s denies it."), "{text}");
        assert!(text.ends_with("only be approved at the console (`agentctl approve coord a43`); `deny a43` works here."), "{text}");
        let refused =
            json!({"id": "a42", "decision": "approve", "message": "approval a42 is not waiting"});
        assert_eq!(
            refused_text("coord", &refused),
            "coord: `approve a42` was not taken: approval a42 is not waiting"
        );
        let decided = json!({"id": "a42", "tool": "Bash", "summary": "ls", "decision": "approved", "by": "chat:U1"});
        assert_eq!(
            decided_text("coord", &decided).unwrap(),
            "coord: a42 approved in chat - Bash `ls`"
        );
        let timeout = json!({"id": "a42", "tool": "Bash", "summary": "ls", "decision": "denied", "by": "timeout", "timeout_s": 90});
        assert_eq!(
            decided_text("coord", &timeout).unwrap(),
            "coord: a42 denied: no answer in 90 s - Bash `ls`"
        );
    }
}
