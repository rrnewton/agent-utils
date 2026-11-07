//! The losslessness gate: prove, row by row, what the archive
//! would lose if the vendor logs went: `py/wrkviz/losslessness.py`
//! at parity.
//!
//! It re-enumerates the vendor rows independently, one file at a
//! time, and requires every single one to fall under a rule that
//! says in words what became of it, then checks the rule's claim
//! against the archive. Three outcomes matter: **unaccounted**
//! (no rule matched), **unverified** (a rule matched and its claim
//! was false), and **declared lossy** (a rule matched and its
//! claim is that the content is *not* retained; that is the
//! inventory, not a failure). Coverage is Codex only, and that is
//! a choice: uncovered teams are named explicitly in the report
//! rather than omitted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use agent_harness::rollout::{complete_prefix, content_parts};
use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::codex::{canonical_records, complete_records, textual};
use crate::json::{as_array, as_int, as_object, as_string, py_repr_str, read_json};
use crate::payloads::{resolve_payloads, verify_payload_store};
use crate::team_data::{PayloadRef, TeamData};

/// The audit report schema version.
pub const AUDIT_SCHEMA_VERSION: i64 = 1;

const SAMPLE_LIMIT: usize = 8;
const EXCERPT: usize = 60;
const RETAINED: &[&str] = &["record", "payload", "state", "duplicate", "redacted"];
const OUTPUT_TYPES: &[&str] = &["custom_tool_call_output", "function_call_output"];

#[derive(Clone, Copy, PartialEq)]
enum Verification {
    None,
    Line,
    PromptText,
    ContentText,
    ObjectiveText,
    InputPayload,
    OutputPayload,
    ModelId,
    CorpusId,
    ParentThread,
}

/// One declared account of what happens to a class of vendor rows.
#[derive(Clone, Copy)]
struct VendorRule {
    name: &'static str,
    top_type: &'static str,
    payload_type: Option<&'static str>,
    disposition: &'static str,
    verification: Verification,
    reason: &'static str,
    id_path: &'static [&'static str],
    selector: Option<(&'static [&'static str], &'static [&'static str])>,
}

impl VendorRule {
    fn blocks_deletion(&self) -> bool {
        !RETAINED.contains(&self.disposition)
    }
}

const fn rule(
    name: &'static str,
    top_type: &'static str,
    payload_type: Option<&'static str>,
    disposition: &'static str,
    verification: Verification,
    reason: &'static str,
) -> VendorRule {
    VendorRule {
        name,
        top_type,
        payload_type,
        disposition,
        verification,
        reason,
        id_path: &[],
        selector: None,
    }
}

/// Every Codex row shape observed across the vendor corpus, and
/// what becomes of each. Reasons are the oracle's, verbatim.
const CODEX_RULES: &[VendorRule] = &[
    VendorRule {
        ..rule(
            "session-metadata",
            "session_meta",
            None,
            "redacted",
            Verification::None,
            "the agent record keeps this row's thread id, parent, agent path, depth and start \
             instant; its `cwd` and any credential embedded in its `git.repository_url` are \
             deliberately removed by `pipeline._archive_team` and are pinned as removed by \
             test_ingest_never_persists_cwd_or_repository_credentials. This is a policy loss on \
             content the archive does not want, not a gap in what it can hold",
        )
    },
    VendorRule {
        ..rule(
            "turn-context",
            "turn_context",
            None,
            "state",
            Verification::None,
            "sets the reader's current turn id and carries no content; every turn it names is \
             recorded by the task_started row that follows it",
        )
    },
    VendorRule {
        id_path: &["turn_id"],
        ..rule(
            "turn-start",
            "event_msg",
            Some("task_started"),
            "record",
            Verification::ModelId,
            "becomes a Turn record",
        )
    },
    VendorRule {
        id_path: &["turn_id"],
        ..rule(
            "turn-complete",
            "event_msg",
            Some("task_complete"),
            "record",
            Verification::ModelId,
            "closes the Turn record with its status, end instant and last agent message",
        )
    },
    VendorRule {
        id_path: &["turn_id"],
        ..rule(
            "turn-aborted",
            "event_msg",
            Some("turn_aborted"),
            "record",
            Verification::ModelId,
            "closes the Turn record as aborted",
        )
    },
    VendorRule {
        ..rule(
            "user-prompt",
            "event_msg",
            Some("user_message"),
            "record",
            Verification::PromptText,
            "becomes a user_prompt Event carrying the message verbatim",
        )
    },
    VendorRule {
        selector: Some((&["role"], &["assistant"])),
        ..rule(
            "assistant-message",
            "response_item",
            Some("message"),
            "record",
            Verification::ContentText,
            "becomes an assistant_message Event carrying the text, or the encrypted content when \
             the provider withheld it",
        )
    },
    VendorRule {
        ..rule(
            "non-assistant-message",
            "response_item",
            Some("message"),
            "dropped",
            Verification::None,
            "a `developer` or `user` role message in the response stream, which the reader skips: \
             its guard is `payload.role == 'assistant'` and there is no else branch. 714 of the \
             1,222 message rows in one measured team. The `user` ones are largely restated by the \
             event_msg/user_message row for the same turn, which *is* recorded; the `developer` \
             ones are the system and instruction prompt the agent actually ran under, and they \
             are nowhere else in the archive",
        )
    },
    VendorRule {
        ..rule(
            "inter-agent-message",
            "response_item",
            Some("agent_message"),
            "record",
            Verification::ContentText,
            "becomes an inter_agent_message Event carrying the text, author and recipient",
        )
    },
    VendorRule {
        ..rule(
            "agent-message-event",
            "event_msg",
            Some("agent_message"),
            "dropped",
            Verification::None,
            "94,074 rows -- the second most numerous shape in the corpus -- and not one of them \
             reaches the model. The reader has an `agent_message` branch, but a `top_type != \
             'response_item': continue` guard sits above it, so this branch is unreachable for an \
             event_msg row; and even if it were reached it reads text from `payload.content` \
             while this row carries it in `payload.message`. The bodies are the agents' own \
             structured adjudications -- risk level, authorization, outcome, rationale -- and \
             they exist only here",
        )
    },
    VendorRule {
        ..rule(
            "subagent-activity",
            "event_msg",
            Some("sub_agent_activity"),
            "record",
            Verification::Line,
            "becomes a spawn or interaction Edge between coordinator and subagent. `line` is the \
             whole claim and it is enough, because the row carries no free text: its fields are \
             `agent_path`, `agent_thread_id`, `event_id`, `kind` and `occurred_at_ms`, all of \
             which are participants and instants rather than content",
        )
    },
    VendorRule {
        ..rule(
            "context-compaction",
            "event_msg",
            Some("context_compacted"),
            "record",
            Verification::Line,
            "becomes a context_compacted Event marking where history was summarized away. The \
             vendor row is `{'type': 'context_compacted'}` and nothing else -- measured across \
             the corpus -- so a citation is the entire content of the row and `line` proves it. \
             What the compaction *replaced* the history with is a separate `compacted` row, \
             inventoried under `compaction-replacement` as the loss it is",
        )
    },
    VendorRule {
        ..rule(
            "thread-goal",
            "event_msg",
            Some("thread_goal_updated"),
            "record",
            Verification::ObjectiveText,
            "becomes a goal_updated Event whose text carries the goal's objective verbatim. The \
             claim is a substring and not an equality because the reader renders status and \
             objective into one line; equality would be a claim about the rendering, and this rule \
             is only entitled to claim that the objective itself survived",
        )
    },
    VendorRule {
        ..rule(
            "tool-call",
            "response_item",
            Some("custom_tool_call"),
            "payload",
            Verification::InputPayload,
            "becomes a ToolCall; its arguments are stored verbatim in the content-addressed \
             payload tree and referenced by digest from raw/team.json",
        )
    },
    VendorRule {
        ..rule(
            "tool-call",
            "response_item",
            Some("function_call"),
            "payload",
            Verification::InputPayload,
            "becomes a ToolCall; its arguments are stored verbatim in the content-addressed \
             payload tree and referenced by digest from raw/team.json",
        )
    },
    VendorRule {
        ..rule(
            "tool-output",
            "response_item",
            Some("custom_tool_call_output"),
            "payload",
            Verification::OutputPayload,
            "completes its ToolCall; the output is stored verbatim in the content-addressed \
             payload tree. This is the single largest thing the archive used to throw away",
        )
    },
    VendorRule {
        ..rule(
            "tool-output",
            "response_item",
            Some("function_call_output"),
            "payload",
            Verification::OutputPayload,
            "completes its ToolCall; the output is stored verbatim in the content-addressed \
             payload tree. This is the single largest thing the archive used to throw away",
        )
    },
    VendorRule {
        id_path: &["item", "id"],
        selector: Some((
            &["item", "type"],
            &[
                "AgentMessage",
                "SubAgentActivity",
                "CollabAgentToolCall",
                "Reasoning",
            ],
        )),
        ..rule(
            "item-completed-duplicate",
            "event_msg",
            Some("item_completed"),
            "duplicate",
            Verification::CorpusId,
            "the streaming UI's restatement of an item the canonical response_item stream already \
             carried, joined on the item id it names -- which resolves for exactly these four \
             item types and is checked here",
        )
    },
    VendorRule {
        id_path: &["turn_id"],
        selector: Some((&["item", "type"], &["CommandExecution"])),
        ..rule(
            "item-completed-command",
            "event_msg",
            Some("item_completed"),
            "partial",
            Verification::ModelId,
            "the executed-command item, and the largest single thing in the corpus: 1,828 rows \
             and 82.2 MiB in one 125 MiB team, because it carries the same output three times \
             over as `stdout`, `aggregated_output` and `formatted_output`. The command and its \
             output text are now in the tool-call and tool-output payloads, so most of this row \
             is genuinely duplicated -- but `exit_code`, `duration`, `cwd`, `process_id`, \
             `parsed_cmd` and the stdout/stderr split exist only here, and its `exec-<uuid>` id \
             is in a namespace nothing else in the model uses, so the join is made at turn \
             granularity and that is all this rule claims. Its byte total is the whole row and is \
             therefore an upper bound on what is actually at risk",
        )
    },
    VendorRule {
        id_path: &["turn_id"],
        selector: Some((&["item", "type"], &["FileChange"])),
        ..rule(
            "item-completed-file-change",
            "event_msg",
            Some("item_completed"),
            "dropped",
            Verification::ModelId,
            "the applied patch again, as the UI rendered it: per-file add/update/delete with \
             complete new file contents. Where the session also wrote a patch_apply_end row the \
             two carry the same bodies and this inventory counts them twice -- but the measured \
             team has 116 of these and no patch_apply_end rows at all, so for that team this is \
             the only copy of the patches the agents wrote",
        )
    },
    VendorRule {
        id_path: &["turn_id"],
        ..rule(
            "item-completed-restatement",
            "event_msg",
            Some("item_completed"),
            "duplicate",
            Verification::ModelId,
            "the same streaming restatement for the remaining item types -- UserMessage and \
             ContextCompaction -- whose UI-side id is not the canonical one, so the join is made \
             at turn granularity. Both restate a row that is recorded: user-prompt and \
             context-compaction respectively",
        )
    },
    VendorRule {
        ..rule(
            "imported-turn-prefix",
            "",
            None,
            "duplicate",
            Verification::ParentThread,
            "precedes the first turn this subagent session owns. Codex writes the parent's \
             history into the child's rollout before the child's own work begins, and \
             `codex._canonical_records` cuts exactly there; these rows are canonical in the \
             parent's own file. The check is that the parent session really is an agent in this \
             archive, because that -- not the individual row -- is what makes 'canonical \
             elsewhere' true",
        )
    },
    VendorRule {
        ..rule(
            "superseded-tool-output",
            "",
            None,
            "dropped",
            Verification::None,
            "an interim output for a tool call that emitted more than one. A long-running command \
             that streams progress writes a `*_call_output` row per update, and the reader assigns \
             `tool_builder.output_text` unconditionally, so only the last one survives and the \
             payload store -- which inherits the model's one-output-per-call shape -- stores only \
             that. Rare and real: 4 of the 38,134 output rows in one measured 1.1 GiB team, all on \
             a single call reporting '5/20' through '20/20 exact-lease actions verified'. Fixing \
             it means the ToolCall carrying a sequence of outputs, which is why it is inventoried \
             here instead. Without this rule these rows read as UNVERIFIED, and a gate that is \
             permanently red for a known cause stops being read at all",
        )
    },
    VendorRule {
        ..rule(
            "unbounded-session",
            "",
            None,
            "dropped",
            Verification::None,
            "every row of a subagent session in which `codex._canonical_records` found no \
             boundary at all: no incoming agent_message naming this agent path, and no \
             task_started at or after the session's own timestamp. The reader takes the agent \
             seed from line 1 and nothing else, so the whole session body is lost -- 2 of the 48 \
             sessions in one measured team. This is a rule rather than an unaccounted row \
             because the cause is known and named; it is `dropped` because the consequence is \
             the same as if it were not",
        )
    },
    VendorRule {
        ..rule(
            "token-count",
            "event_msg",
            Some("token_count"),
            "dropped",
            Verification::None,
            "per-turn context and token usage, plus rate-limit state. 817,440 rows -- more than \
             any other single shape -- and nothing in the model holds any of it. This is the \
             archive's whole record of what each agent's context cost",
        )
    },
    VendorRule {
        ..rule(
            "patch-apply",
            "event_msg",
            Some("patch_apply_end"),
            "dropped",
            Verification::None,
            "the applied patch: per-file add/update/delete with the complete new file contents, \
             plus the apply's stdout and stderr. 38,571 rows. Its `call_id` is an `exec-<uuid>` \
             in a different namespace from the `call_...` ids on tool calls, so it cannot simply \
             be attached to one as a third payload slot -- which is why it is inventoried here \
             rather than fixed alongside tool stdout",
        )
    },
    VendorRule {
        ..rule(
            "reasoning",
            "response_item",
            Some("reasoning"),
            "dropped",
            Verification::None,
            "the model's reasoning item: a summary array and, usually, an encrypted_content blob. \
             132,817 rows. The Event family already carries encrypted content for messages, so \
             there is somewhere for this to go; nothing puts it there today",
        )
    },
    VendorRule {
        ..rule(
            "thread-settings",
            "event_msg",
            Some("thread_settings_applied"),
            "dropped",
            Verification::None,
            "the model, provider, service tier, approval policy and full sandbox permission \
             profile in force from this point. 6,046 rows, and the only record of what each \
             agent was actually allowed to do",
        )
    },
    VendorRule {
        ..rule(
            "mcp-tool-call",
            "event_msg",
            Some("mcp_tool_call_end"),
            "dropped",
            Verification::None,
            "an MCP tool invocation's completion; 2 rows in the corpus and no reader for them",
        )
    },
    VendorRule {
        ..rule(
            "world-state",
            "world_state",
            None,
            "dropped",
            Verification::None,
            "the agent instruction context in force -- AGENTS.md text and the directory it came \
             from. 17,955 rows, mostly repeated verbatim, and none of it reaches the model",
        )
    },
    VendorRule {
        ..rule(
            "compaction-replacement",
            "compacted",
            None,
            "dropped",
            Verification::None,
            "the replacement history a compaction installed, which is the only copy of what the \
             conversation looked like after it. 4,431 rows. The context_compacted Event records \
             that a compaction happened, not what survived it",
        )
    },
    VendorRule {
        ..rule(
            "inter-agent-metadata",
            "inter_agent_communication_metadata",
            None,
            "dropped",
            Verification::None,
            "a one-field trigger flag accompanying an inter-agent message; 9,824 rows",
        )
    },
];

fn positional(name: &str) -> &'static VendorRule {
    CODEX_RULES.iter().find(|item| item.name == name).unwrap()
}

fn shape_rules() -> HashMap<(String, Option<String>), Vec<&'static VendorRule>> {
    let mut shapes: HashMap<(String, Option<String>), Vec<&'static VendorRule>> = HashMap::new();
    for item in CODEX_RULES {
        if matches!(
            item.name,
            "imported-turn-prefix" | "unbounded-session" | "superseded-tool-output"
        ) {
            continue;
        }
        shapes
            .entry((
                item.top_type.to_string(),
                item.payload_type.map(str::to_string),
            ))
            .or_default()
            .push(item);
    }
    shapes
}

fn str_of(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn map_of(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn identifier(payload: &Map<String, Value>, path: &[&str]) -> Option<String> {
    let mut current: Option<Value> = Some(Value::Object(payload.clone()));
    for key in path {
        match current {
            Some(Value::Object(map)) => current = map.get(*key).cloned(),
            _ => return None,
        }
    }
    current.as_ref().and_then(Value::as_str).map(str::to_owned)
}

fn select<'a>(
    rules: Option<&Vec<&'a VendorRule>>,
    payload: &Map<String, Value>,
) -> Option<&'a VendorRule> {
    let rules = rules?;
    for item in rules {
        match item.selector {
            None => return Some(item),
            Some((path, accepted)) => {
                if identifier(payload, path).is_some_and(|held| accepted.contains(&held.as_str())) {
                    return Some(item);
                }
            }
        }
    }
    None
}

/// Undo `codex._scoped_id` so a vendor row's raw id can be looked
/// up in the model. Both components are length-prefixed, which
/// makes the parse exact: a string that merely looks like the
/// prefix fails one of the checks and is returned unchanged.
pub fn unscope_codex_id(value: &str) -> String {
    let Some(rest) = value.strip_prefix("codex-continuation-") else {
        return value.to_string();
    };
    let Some((lineage_length, remainder)) = rest.split_once('-') else {
        return value.to_string();
    };
    if lineage_length.is_empty() || !lineage_length.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.to_string();
    }
    let Ok(skip) = lineage_length.parse::<usize>() else {
        return value.to_string();
    };
    let chars: Vec<char> = remainder.chars().collect();
    if chars.len() <= skip || chars[skip] != '-' {
        return value.to_string();
    }
    let tail: String = chars[skip + 1..].iter().collect();
    let Some((raw_length, raw)) = tail.split_once('-') else {
        return value.to_string();
    };
    if raw_length.is_empty() || !raw_length.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.to_string();
    }
    let Ok(expected) = raw_length.parse::<usize>() else {
        return value.to_string();
    };
    if raw.chars().count() != expected {
        return value.to_string();
    }
    raw.to_string()
}

/// One vendor row the audit could not account for, or could not
/// confirm.
#[derive(Clone, Debug)]
pub struct RowFinding {
    /// Snapshot path relative to the snapshot root.
    pub source_path: String,
    /// 1-based line in that file.
    pub line: i64,
    /// The row's top-level `type`, when a string.
    pub top_type: Option<String>,
    /// The payload's `type`, when a string.
    pub payload_type: Option<String>,
    /// Raw row byte count.
    pub vendor_bytes: i64,
    /// What is wrong, in words.
    pub detail: String,
}

impl RowFinding {
    fn to_json_obj(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "source_path".into(),
            Value::String(self.source_path.clone()),
        );
        out.insert("line".into(), self.line.into());
        out.insert(
            "top_type".into(),
            self.top_type.clone().map_or(Value::Null, Value::String),
        );
        out.insert(
            "payload_type".into(),
            self.payload_type.clone().map_or(Value::Null, Value::String),
        );
        out.insert("vendor_bytes".into(), self.vendor_bytes.into());
        out.insert("detail".into(), Value::String(self.detail.clone()));
        Value::Object(out)
    }
}

/// How much of the corpus one rule accounted for.
#[derive(Clone, Debug)]
pub struct RuleTally {
    /// Rule name.
    pub name: String,
    /// What became of the rows.
    pub disposition: String,
    /// Row count.
    pub rows: i64,
    /// Whole-row vendor bytes.
    pub vendor_bytes: i64,
    /// Rows whose claim did not hold.
    pub unverified: i64,
    /// Whether these rows keep the vendor file load-bearing.
    pub blocks_deletion: bool,
    /// The rule's reason, in words.
    pub reason: String,
}

impl RuleTally {
    fn to_json_obj(&self) -> Value {
        let mut out = Map::new();
        out.insert("rule".into(), Value::String(self.name.clone()));
        out.insert(
            "disposition".into(),
            Value::String(self.disposition.clone()),
        );
        out.insert("rows".into(), self.rows.into());
        out.insert("vendor_bytes".into(), self.vendor_bytes.into());
        out.insert("unverified".into(), self.unverified.into());
        out.insert("blocks_deletion".into(), Value::Bool(self.blocks_deletion));
        out.insert("reason".into(), Value::String(self.reason.clone()));
        Value::Object(out)
    }
}

/// What one team's vendor corpus contains, and what the archive
/// would keep without it.
#[derive(Clone, Debug)]
pub struct LosslessnessReport {
    /// Team slug.
    pub team_slug: String,
    /// Provider named by the source manifest.
    pub provider: String,
    /// Whether a rule table covers this provider.
    pub covered: bool,
    /// Vendor files enumerated.
    pub vendor_files: i64,
    /// Vendor rows enumerated.
    pub vendor_rows: i64,
    /// Vendor bytes enumerated.
    pub vendor_bytes: i64,
    /// Rows whose content is entirely absent from the archive.
    pub absent_rows: i64,
    /// Whole-row bytes for those rows.
    pub absent_bytes: i64,
    /// Rows only part of whose content is held.
    pub partial_rows: i64,
    /// Whole-row bytes for those rows (an upper bound).
    pub partial_bytes: i64,
    /// Per-rule tallies, largest first.
    pub tallies: Vec<RuleTally>,
    /// Sampled unaccounted rows (capped).
    pub unaccounted: Vec<RowFinding>,
    /// Total unaccounted rows.
    pub unaccounted_rows: i64,
    /// Sampled unverified rows (capped).
    pub unverified: Vec<RowFinding>,
    /// Total unverified rows.
    pub unverified_rows: i64,
    /// Payload-store discrepancies.
    pub payload_problems: Vec<String>,
    /// Ways the vendor tree is not the recorded tree.
    pub source_problems: Vec<String>,
    /// Why this team was not covered, when it was not.
    pub skipped: Option<String>,
}

impl LosslessnessReport {
    /// Whether every row was accounted for and every claim held.
    pub fn sound(&self) -> bool {
        self.covered
            && self.unaccounted_rows == 0
            && self.unverified_rows == 0
            && self.payload_problems.is_empty()
            && self.source_problems.is_empty()
    }

    /// Whether deleting the vendor snapshots would lose nothing
    /// the archive wants.
    pub fn lossless(&self) -> bool {
        self.sound() && self.absent_rows == 0 && self.partial_rows == 0
    }

    fn to_json_obj(&self) -> Value {
        let mut out = Map::new();
        out.insert("team_slug".into(), Value::String(self.team_slug.clone()));
        out.insert("provider".into(), Value::String(self.provider.clone()));
        out.insert("covered".into(), Value::Bool(self.covered));
        out.insert(
            "skipped".into(),
            self.skipped.clone().map_or(Value::Null, Value::String),
        );
        out.insert("sound".into(), Value::Bool(self.sound()));
        out.insert("lossless".into(), Value::Bool(self.lossless()));
        out.insert("vendor_files".into(), self.vendor_files.into());
        out.insert("vendor_rows".into(), self.vendor_rows.into());
        out.insert("vendor_bytes".into(), self.vendor_bytes.into());
        out.insert("absent_rows".into(), self.absent_rows.into());
        out.insert("absent_bytes".into(), self.absent_bytes.into());
        out.insert("partial_rows".into(), self.partial_rows.into());
        out.insert("partial_bytes".into(), self.partial_bytes.into());
        out.insert("unaccounted_rows".into(), self.unaccounted_rows.into());
        out.insert("unverified_rows".into(), self.unverified_rows.into());
        out.insert(
            "rules".into(),
            Value::Array(self.tallies.iter().map(RuleTally::to_json_obj).collect()),
        );
        out.insert(
            "unaccounted_samples".into(),
            Value::Array(
                self.unaccounted
                    .iter()
                    .map(RowFinding::to_json_obj)
                    .collect(),
            ),
        );
        out.insert(
            "unverified_samples".into(),
            Value::Array(
                self.unverified
                    .iter()
                    .map(RowFinding::to_json_obj)
                    .collect(),
            ),
        );
        out.insert(
            "payload_problems".into(),
            Value::Array(
                self.payload_problems
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        out.insert(
            "source_problems".into(),
            Value::Array(
                self.source_problems
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        Value::Object(out)
    }
}

/// Every selected team's verdict, including uncovered ones.
#[derive(Clone, Debug)]
pub struct ArchiveLosslessnessReport {
    /// Per-team reports, in selection order.
    pub reports: Vec<LosslessnessReport>,
}

impl ArchiveLosslessnessReport {
    /// Whether every *covered* team's account of itself held.
    pub fn sound(&self) -> bool {
        self.reports
            .iter()
            .filter(|item| item.covered)
            .all(|item| item.sound())
    }

    /// Whether the whole selection could give up its vendor
    /// snapshots today.
    pub fn lossless(&self) -> bool {
        !self.reports.is_empty() && self.reports.iter().all(|item| item.lossless())
    }

    /// The durable JSON record.
    pub fn to_json_obj(&self) -> Value {
        let mut out = Map::new();
        out.insert("schema_version".into(), AUDIT_SCHEMA_VERSION.into());
        out.insert("tool".into(), Value::String("wrkviz".into()));
        out.insert("sound".into(), Value::Bool(self.sound()));
        out.insert("lossless".into(), Value::Bool(self.lossless()));
        out.insert(
            "teams".into(),
            Value::Array(
                self.reports
                    .iter()
                    .map(LosslessnessReport::to_json_obj)
                    .collect(),
            ),
        );
        Value::Object(out)
    }
}

struct FileContext {
    thread_id: String,
    parent_thread_id: Option<String>,
}

type EventContent = HashMap<(String, i64), Vec<(Option<String>, Option<String>)>>;

/// Everything the rule checks need from `raw/team.json`, joined
/// the way vendor rows join.
struct ArchiveIndex {
    lines: HashSet<(String, i64)>,
    event_content: EventContent,
    identifiers: HashSet<String>,
    input_payload_by_call: HashMap<String, Option<String>>,
    output_payload_by_call: HashMap<String, Option<String>>,
    resolved: HashMap<String, String>,
}

fn build_index(archive: &Path, team: &TeamData) -> Result<ArchiveIndex, String> {
    let mut lines: HashSet<(String, i64)> = HashSet::new();
    let mut content: EventContent = HashMap::new();
    for event in &team.events {
        lines.insert((event.thread_id.clone(), event.source_line));
        content
            .entry((event.thread_id.clone(), event.source_line))
            .or_default()
            .push((event.text.clone(), event.encrypted_content.clone()));
    }
    for tool in &team.tool_calls {
        lines.insert((tool.thread_id.clone(), tool.source_line));
    }
    for edge in &team.edges {
        lines.insert((edge.from_thread_id.clone(), edge.source_line));
    }
    let mut identifiers: HashSet<String> = HashSet::new();
    for event in &team.events {
        identifiers.insert(unscope_codex_id(&event.event_id));
    }
    for turn in &team.turns {
        identifiers.insert(unscope_codex_id(&turn.turn_id));
    }
    for tool in &team.tool_calls {
        identifiers.insert(unscope_codex_id(&tool.call_id));
        if let Some(item_id) = &tool.item_id {
            identifiers.insert(unscope_codex_id(item_id));
        }
    }
    for edge in &team.edges {
        identifiers.insert(unscope_codex_id(&edge.edge_id));
    }
    for agent in &team.agents {
        identifiers.insert(agent.thread_id.clone());
    }
    let mut inputs: HashMap<String, Option<String>> = HashMap::new();
    let mut outputs: HashMap<String, Option<String>> = HashMap::new();
    let mut refs: Vec<PayloadRef> = Vec::new();
    for tool in &team.tool_calls {
        let raw_call_id = unscope_codex_id(&tool.call_id);
        inputs.insert(
            raw_call_id.clone(),
            tool.input_payload.as_ref().map(|item| item.sha256.clone()),
        );
        outputs.insert(
            raw_call_id,
            tool.output_payload.as_ref().map(|item| item.sha256.clone()),
        );
        if let Some(item) = &tool.input_payload {
            refs.push(item.clone());
        }
        if let Some(item) = &tool.output_payload {
            refs.push(item.clone());
        }
    }
    let team_root = crate::build_store::team_build_root(archive, &team.team_slug)?;
    let resolved = resolve_payloads(&team_root.join("payloads"), &refs)?;
    Ok(ArchiveIndex {
        lines,
        event_content: content,
        identifiers,
        input_payload_by_call: inputs,
        output_payload_by_call: outputs,
        resolved: resolved.into_iter().collect(),
    })
}

fn excerpt(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "nothing".to_string();
    };
    if value.chars().count() <= EXCERPT {
        return py_repr_str(value);
    }
    let head: String = value.chars().take(EXCERPT).collect();
    format!(
        "{}... ({} characters)",
        py_repr_str(&head),
        value.chars().count()
    )
}

enum Verdict {
    Ok,
    Fail(String),
    Deferred(String),
}

fn verify_event_content(
    context: &FileContext,
    line: i64,
    index: &ArchiveIndex,
    expected: &(Option<String>, Option<String>),
) -> Verdict {
    let found: &[(Option<String>, Option<String>)] = index
        .event_content
        .get(&(context.thread_id.clone(), line))
        .map_or(&[], Vec::as_slice);
    if found.is_empty() {
        return Verdict::Fail("no event in raw/team.json cites this line".to_string());
    }
    if found.contains(expected) {
        return Verdict::Ok;
    }
    let (text, encrypted) = expected;
    let (held_text, held_encrypted) = &found[0];
    if text.is_some() && held_text != text {
        return Verdict::Fail(format!(
            "the event at this line holds text {}, the row carries {}",
            excerpt(held_text.as_deref()),
            excerpt(text.as_deref())
        ));
    }
    Verdict::Fail(format!(
        "the event at this line holds encrypted content {}, the row carries {}",
        excerpt(held_encrypted.as_deref()),
        excerpt(encrypted.as_deref())
    ))
}

fn verify_payload_row(
    payload: &Map<String, Value>,
    index: &ArchiveIndex,
    is_output: bool,
) -> Verdict {
    let Some(call_id) = str_of(payload.get("call_id")) else {
        return Verdict::Fail(
            "the row names no call_id, so the reader could not have built a tool call from it"
                .to_string(),
        );
    };
    let slot = if is_output {
        &index.output_payload_by_call
    } else {
        &index.input_payload_by_call
    };
    let Some(digest) = slot.get(&call_id) else {
        return Verdict::Fail(format!(
            "no tool call in raw/team.json corresponds to call_id {}",
            py_repr_str(&call_id)
        ));
    };
    let expected = if is_output {
        textual(payload.get("output"))
    } else {
        str_of(payload.get("input")).or_else(|| str_of(payload.get("arguments")))
    };
    let Some(expected) = expected else {
        return if digest.is_none() {
            Verdict::Ok
        } else {
            Verdict::Fail(format!(
                "call_id {} references stored text for a row that carries none",
                py_repr_str(&call_id)
            ))
        };
    };
    let Some(digest) = digest else {
        return Verdict::Fail(format!(
            "call_id {} has no stored payload for text this row carries",
            py_repr_str(&call_id)
        ));
    };
    let Some(stored) = index.resolved.get(digest) else {
        return Verdict::Fail(format!(
            "payload {digest} is referenced by call_id {} but absent from the store",
            py_repr_str(&call_id)
        ));
    };
    if *stored != expected {
        return Verdict::Fail(format!(
            "payload {digest} does not match the text this row carries"
        ));
    }
    Verdict::Ok
}

fn verify(
    item: &VendorRule,
    context: &FileContext,
    line: i64,
    payload: &Map<String, Value>,
    index: &ArchiveIndex,
) -> Verdict {
    match item.verification {
        Verification::None => Verdict::Ok,
        Verification::Line => {
            if index.lines.contains(&(context.thread_id.clone(), line)) {
                Verdict::Ok
            } else {
                Verdict::Fail(
                    "no event, tool call or edge in raw/team.json cites this line".to_string(),
                )
            }
        }
        Verification::PromptText => verify_event_content(
            context,
            line,
            index,
            &(str_of(payload.get("message")), None),
        ),
        Verification::ContentText => {
            let (text, encrypted, _availability) = content_parts(payload);
            verify_event_content(context, line, index, &(text, encrypted))
        }
        Verification::ObjectiveText => {
            let objective = str_of(map_of(payload.get("goal")).get("objective"));
            let found: &[(Option<String>, Option<String>)] = index
                .event_content
                .get(&(context.thread_id.clone(), line))
                .map_or(&[], Vec::as_slice);
            if found.is_empty() {
                return Verdict::Fail("no event in raw/team.json cites this line".to_string());
            }
            let Some(objective) = objective else {
                return Verdict::Ok;
            };
            if found
                .iter()
                .any(|(held, _)| held.as_ref().is_some_and(|held| held.contains(&objective)))
            {
                return Verdict::Ok;
            }
            Verdict::Fail(format!(
                "the event at this line does not carry the objective {}; it holds {}",
                excerpt(Some(&objective)),
                excerpt(found[0].0.as_deref())
            ))
        }
        Verification::InputPayload => verify_payload_row(payload, index, false),
        Verification::OutputPayload => verify_payload_row(payload, index, true),
        Verification::ParentThread => {
            let Some(parent) = &context.parent_thread_id else {
                return Verdict::Fail(
                    "the session declares no parent, so nothing replayed its history into it"
                        .to_string(),
                );
            };
            if index.identifiers.contains(parent) {
                Verdict::Ok
            } else {
                Verdict::Fail(format!(
                    "parent session {} is not an agent in raw/team.json, so these replayed rows \
                     are canonical nowhere in this archive",
                    py_repr_str(parent)
                ))
            }
        }
        Verification::ModelId | Verification::CorpusId => {
            let Some(held) = identifier(payload, item.id_path) else {
                return Verdict::Fail(format!(
                    "the row carries no {} to check against the model",
                    item.id_path.join(".")
                ));
            };
            if index.identifiers.contains(&unscope_codex_id(&held)) {
                return Verdict::Ok;
            }
            if item.verification == Verification::CorpusId {
                return Verdict::Deferred(held);
            }
            Verdict::Fail(format!(
                "identifier {} names nothing in raw/team.json",
                py_repr_str(&held)
            ))
        }
    }
}

struct RecordedSource {
    snapshot_path: String,
    copied_bytes: i64,
    sha256: String,
}

fn recorded_sources(
    manifest: &Map<String, Value>,
    where_: &str,
) -> Result<Vec<RecordedSource>, String> {
    let mut recorded = Vec::new();
    let items = as_array(
        manifest.get("sources").unwrap_or(&Value::Null),
        &format!("{where_}.sources"),
    )?;
    for (index, item) in items.iter().enumerate() {
        let entry = as_object(item, &format!("{where_}.sources[{index}]"))?;
        recorded.push(RecordedSource {
            snapshot_path: as_string(
                entry.get("snapshot_path").unwrap_or(&Value::Null),
                &format!("{where_}.sources[{index}].snapshot_path"),
            )?
            .to_string(),
            copied_bytes: as_int(
                entry.get("copied_bytes").unwrap_or(&Value::Null),
                &format!("{where_}.sources[{index}].copied_bytes"),
            )?,
            sha256: as_string(
                entry.get("sha256").unwrap_or(&Value::Null),
                &format!("{where_}.sources[{index}].sha256"),
            )?
            .to_string(),
        });
    }
    Ok(recorded)
}

fn lineage_roots(manifest: &Map<String, Value>, where_: &str) -> Result<HashSet<String>, String> {
    let mut roots: HashSet<String> = HashSet::new();
    roots.insert(
        as_string(
            manifest.get("root_thread_id").unwrap_or(&Value::Null),
            &format!("{where_}.root_thread_id"),
        )?
        .to_string(),
    );
    if let Some(raw_links) = manifest.get("continuation_sessions") {
        if !raw_links.is_null() {
            let links = as_array(raw_links, &format!("{where_}.continuation_sessions"))?;
            for (index, link) in links.iter().enumerate() {
                let entry = as_object(link, &format!("{where_}.continuation_sessions[{index}]"))?;
                roots.insert(
                    as_string(
                        entry.get("thread_id").unwrap_or(&Value::Null),
                        &format!("{where_}.continuation_sessions"),
                    )?
                    .to_string(),
                );
            }
        }
    }
    Ok(roots)
}

fn parent_thread_id(metadata: &Map<String, Value>) -> Option<String> {
    let subagent = map_of(map_of(metadata.get("source")).get("subagent"));
    let spawn = map_of(subagent.get("thread_spawn"));
    match str_of(metadata.get("parent_thread_id")) {
        Some(held) if !held.is_empty() => Some(held),
        _ => str_of(spawn.get("parent_thread_id")),
    }
}

fn collect_jsonl(root: &Path, dir: &Path, out: &mut Vec<(Vec<String>, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_jsonl(root, &path, out);
        } else if file_type.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".jsonl") {
                let relative: Vec<String> = path
                    .strip_prefix(root)
                    .map(|item| {
                        item.components()
                            .map(|part| part.as_os_str().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                out.push((relative, path));
            }
        }
    }
}

/// CPython `raw[:copied_bytes]`, including negative counts
/// slicing from the end.
fn recorded_prefix(raw: &[u8], copied_bytes: i64) -> &[u8] {
    if copied_bytes >= 0 {
        &raw[..copied_bytes as usize]
    } else {
        let keep = (raw.len() as i64 + copied_bytes).max(0) as usize;
        &raw[..keep]
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Default)]
struct Accumulator {
    rows: i64,
    vendor_bytes: i64,
    unverified: i64,
}

struct DeferredCheck {
    rule_name: String,
    identifier: String,
    finding: RowFinding,
}

fn skipped(team_slug: &str, provider: &str, reason: &str) -> LosslessnessReport {
    LosslessnessReport {
        team_slug: team_slug.to_string(),
        provider: provider.to_string(),
        covered: false,
        vendor_files: 0,
        vendor_rows: 0,
        vendor_bytes: 0,
        absent_rows: 0,
        absent_bytes: 0,
        partial_rows: 0,
        partial_bytes: 0,
        tallies: Vec::new(),
        unaccounted: Vec::new(),
        unaccounted_rows: 0,
        unverified: Vec::new(),
        unverified_rows: 0,
        payload_problems: Vec::new(),
        source_problems: Vec::new(),
        skipped: Some(reason.to_string()),
    }
}

/// Account for every row of one Codex team's vendor snapshots
/// against its archive. Read-only: takes no lock, writes nothing.
pub fn audit_codex_losslessness(
    archive: &Path,
    team_slug: &str,
) -> Result<LosslessnessReport, String> {
    let team_root = crate::build_store::team_build_root(archive, team_slug)?;
    let manifest_path = team_root.join("raw").join("source-manifest.json");
    if !manifest_path.is_file() {
        return Ok(skipped(
            team_slug,
            "unknown",
            "the team has no raw/source-manifest.json",
        ));
    }
    let manifest_path_text = manifest_path.display().to_string();
    let manifest_value = read_json(&manifest_path)?;
    let manifest = as_object(&manifest_value, &manifest_path_text)?;
    let provider = as_string(
        manifest.get("provider").unwrap_or(&Value::Null),
        &format!("{manifest_path_text}.provider"),
    )?
    .to_string();
    if provider != "codex" {
        return Ok(skipped(
            team_slug,
            &provider,
            &format!(
                "no rule table exists for provider {}; only codex is covered",
                py_repr_str(&provider)
            ),
        ));
    }
    let snapshot_root =
        crate::snapshot_store::resolve_snapshot_root(archive, team_slug, None)?.root;
    if !snapshot_root.is_dir() {
        return Ok(skipped(
            team_slug,
            &provider,
            "the vendor snapshots are already absent, so there is nothing left to compare \
             the archive against",
        ));
    }
    let team = crate::pipeline::load_archived_team(archive, team_slug)?;
    let index = build_index(archive, &team)?;
    let payload_problems = verify_payload_store(&team_root.join("payloads"))?;
    let roots = lineage_roots(manifest, &manifest_path_text)?;
    let recorded = recorded_sources(manifest, &manifest_path_text)?;
    let recorded_by_path: HashMap<&str, &RecordedSource> = recorded
        .iter()
        .map(|item| (item.snapshot_path.as_str(), item))
        .collect();
    let mut observed_paths: HashSet<String> = HashSet::new();
    let mut source_problems: Vec<String> = Vec::new();
    let shapes = shape_rules();
    let rules_by_name: HashMap<&str, &VendorRule> =
        CODEX_RULES.iter().map(|item| (item.name, item)).collect();
    let prefix_rule = positional("imported-turn-prefix");
    let unbounded_rule = positional("unbounded-session");
    let superseded_rule = positional("superseded-tool-output");
    let mut tallies: HashMap<String, Accumulator> = HashMap::new();
    let mut unaccounted: Vec<RowFinding> = Vec::new();
    let mut unverified: Vec<RowFinding> = Vec::new();
    let mut deferred: Vec<DeferredCheck> = Vec::new();
    let mut corpus_ids: HashSet<String> = HashSet::new();
    let mut unaccounted_rows = 0i64;
    let mut unverified_rows = 0i64;
    let mut vendor_files = 0i64;
    let mut vendor_rows = 0i64;
    let mut vendor_bytes = 0i64;

    let mut files: Vec<(Vec<String>, PathBuf)> = Vec::new();
    collect_jsonl(&snapshot_root, &snapshot_root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    for (components, path) in &files {
        vendor_files += 1;
        let relative = components.join("/");
        let raw = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
        observed_paths.insert(relative.clone());
        match recorded_by_path.get(relative.as_str()) {
            None => source_problems.push(format!(
                "{relative}: present in source_snapshots/ but not recorded in \
                 raw/source-manifest.json"
            )),
            Some(item) if (raw.len() as i64) < item.copied_bytes => source_problems.push(format!(
                "{relative}: {} bytes on disk, the manifest records {}",
                raw.len(),
                item.copied_bytes
            )),
            Some(item) if sha256_hex(recorded_prefix(&raw, item.copied_bytes)) != item.sha256 => {
                source_problems.push(format!(
                    "{relative}: bytes do not match the digest recorded in \
                     raw/source-manifest.json"
                ));
            }
            _ => {}
        }
        let records = complete_records(complete_prefix(&raw), &relative)?;
        if records.is_empty() {
            continue;
        }
        let metadata = records[0].payload();
        let thread_id = str_of(metadata.get("id")).unwrap_or_default();
        let context = FileContext {
            parent_thread_id: parent_thread_id(&metadata),
            thread_id: thread_id.clone(),
        };
        let canonical = canonical_records(
            &records,
            &metadata,
            if roots.contains(&thread_id) {
                &thread_id
            } else {
                ""
            },
        );
        let canonical_start = canonical
            .first()
            .map_or(records.len() as i64 + 1, |item| item.line);
        let unbounded = canonical.is_empty() && records.len() > 1;
        let mut last_output_line: HashMap<String, i64> = HashMap::new();
        for record in &canonical {
            let payload = record.payload();
            if OUTPUT_TYPES.contains(&str_of(payload.get("type")).as_deref().unwrap_or("")) {
                if let Some(call) = str_of(payload.get("call_id")) {
                    last_output_line.insert(call, record.line);
                }
            }
        }
        for record in &records {
            vendor_rows += 1;
            let row_bytes = record.raw.len() as i64;
            vendor_bytes += row_bytes;
            let top_type = record.kind().map(str::to_owned);
            let payload = record.payload();
            let payload_type = str_of(payload.get("type"));
            let matched: Option<&VendorRule> = if record.line == 1
                && top_type.as_deref() == Some("session_meta")
            {
                select(shapes.get(&("session_meta".to_string(), None)), &payload)
            } else if unbounded {
                Some(unbounded_rule)
            } else if record.line < canonical_start {
                Some(prefix_rule)
            } else if top_type.is_none() {
                None
            } else if payload_type
                .as_ref()
                .is_some_and(|held| OUTPUT_TYPES.contains(&held.as_str()))
                && last_output_line.get(str_of(payload.get("call_id")).as_deref().unwrap_or(""))
                    != Some(&record.line)
            {
                Some(superseded_rule)
            } else {
                let top = top_type.clone().unwrap_or_default();
                let candidates = shapes
                    .get(&(top.clone(), payload_type.clone()))
                    .or_else(|| {
                        if payload_type.is_some() {
                            shapes.get(&(top, None))
                        } else {
                            None
                        }
                    });
                select(candidates, &payload)
            };
            if top_type.as_deref() == Some("response_item")
                && record.line >= canonical_start
                && !unbounded
            {
                for key in ["id", "call_id"] {
                    if let Some(value) = str_of(payload.get(key)) {
                        corpus_ids.insert(value);
                    }
                }
            }
            let Some(item) = matched else {
                unaccounted_rows += 1;
                if unaccounted.len() < SAMPLE_LIMIT {
                    unaccounted.push(RowFinding {
                        source_path: relative.clone(),
                        line: record.line,
                        top_type: top_type.clone(),
                        payload_type: payload_type.clone(),
                        vendor_bytes: row_bytes,
                        detail: "no rule in the Codex table accounts for this row shape; add one \
                                 that says what becomes of it before trusting any inventory below"
                            .to_string(),
                    });
                }
                continue;
            };
            let accumulator = tallies.entry(item.name.to_string()).or_default();
            accumulator.rows += 1;
            accumulator.vendor_bytes += row_bytes;
            match verify(item, &context, record.line, &payload, &index) {
                Verdict::Ok => {}
                Verdict::Deferred(held) => {
                    deferred.push(DeferredCheck {
                        rule_name: item.name.to_string(),
                        identifier: held,
                        finding: RowFinding {
                            source_path: relative.clone(),
                            line: record.line,
                            top_type: top_type.clone(),
                            payload_type: payload_type.clone(),
                            vendor_bytes: row_bytes,
                            detail: String::new(),
                        },
                    });
                }
                Verdict::Fail(failure) => {
                    tallies.get_mut(item.name).unwrap().unverified += 1;
                    unverified_rows += 1;
                    if unverified.len() < SAMPLE_LIMIT {
                        unverified.push(RowFinding {
                            source_path: relative.clone(),
                            line: record.line,
                            top_type: top_type.clone(),
                            payload_type: payload_type.clone(),
                            vendor_bytes: row_bytes,
                            detail: format!(
                                "rule {} claims otherwise: {failure}",
                                py_repr_str(item.name)
                            ),
                        });
                    }
                }
            }
        }
    }
    for source in &recorded {
        if !observed_paths.contains(&source.snapshot_path) {
            source_problems.push(format!(
                "{}: recorded in raw/source-manifest.json but absent from source_snapshots/",
                source.snapshot_path
            ));
        }
    }
    for check in deferred {
        if corpus_ids.contains(&check.identifier) {
            continue;
        }
        if let Some(accumulator) = tallies.get_mut(&check.rule_name) {
            accumulator.unverified += 1;
        }
        unverified_rows += 1;
        if unverified.len() < SAMPLE_LIMIT {
            let mut finding = check.finding;
            finding.detail = format!(
                "rule {} claims otherwise: identifier {} appears on no canonical row anywhere \
                 in the corpus and names nothing in raw/team.json, so this row restates nothing",
                py_repr_str(&check.rule_name),
                py_repr_str(&check.identifier)
            );
            unverified.push(finding);
        }
    }
    let mut ordered: Vec<RuleTally> = tallies
        .iter()
        .map(|(name, accumulator)| {
            let item = rules_by_name[name.as_str()];
            RuleTally {
                name: name.clone(),
                disposition: item.disposition.to_string(),
                rows: accumulator.rows,
                vendor_bytes: accumulator.vendor_bytes,
                unverified: accumulator.unverified,
                blocks_deletion: item.blocks_deletion(),
                reason: item.reason.to_string(),
            }
        })
        .collect();
    ordered.sort_by(|left, right| {
        right
            .vendor_bytes
            .cmp(&left.vendor_bytes)
            .then_with(|| left.name.cmp(&right.name))
    });
    let absent_rows: i64 = ordered
        .iter()
        .filter(|tally| tally.disposition == "dropped")
        .map(|tally| tally.rows)
        .sum();
    let absent_bytes: i64 = ordered
        .iter()
        .filter(|tally| tally.disposition == "dropped")
        .map(|tally| tally.vendor_bytes)
        .sum();
    let partial_rows: i64 = ordered
        .iter()
        .filter(|tally| tally.disposition == "partial")
        .map(|tally| tally.rows)
        .sum();
    let partial_bytes: i64 = ordered
        .iter()
        .filter(|tally| tally.disposition == "partial")
        .map(|tally| tally.vendor_bytes)
        .sum();
    source_problems.sort();
    Ok(LosslessnessReport {
        team_slug: team_slug.to_string(),
        provider,
        covered: true,
        vendor_files,
        vendor_rows,
        vendor_bytes,
        absent_rows,
        absent_bytes,
        partial_rows,
        partial_bytes,
        tallies: ordered,
        unaccounted,
        unaccounted_rows,
        unverified,
        unverified_rows,
        payload_problems,
        source_problems,
        skipped: None,
    })
}

/// Audit the selected teams, or every ingested team when the
/// selection is empty. An uncovered team is reported, never
/// omitted.
pub fn audit_archive_losslessness(
    archive: &Path,
    team_slugs: &[String],
) -> Result<ArchiveLosslessnessReport, String> {
    crate::cli::validation_checkpoint();
    let selected: Vec<String> = if team_slugs.is_empty() {
        crate::build_store::ingested_team_slugs(archive)?
    } else {
        team_slugs.to_vec()
    };
    if selected.is_empty() {
        return Err(format!("no ingested teams found in {}", archive.display()));
    }
    let unique: HashSet<&String> = selected.iter().collect();
    if unique.len() != selected.len() {
        return Err("losslessness audit team selection contains duplicates".to_string());
    }
    let mut reports = Vec::new();
    for slug in &selected {
        reports.push(audit_codex_losslessness(archive, slug)?);
    }
    Ok(ArchiveLosslessnessReport { reports })
}

fn mib(value: i64) -> String {
    format!("{:.1} MiB", value as f64 / (1024.0 * 1024.0))
}

fn commafy(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    if value < 0 {
        out.push('-');
    }
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn opt_text(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(|| "None".to_string())
}

/// Render the audit for a terminal.
pub fn format_losslessness_audit(report: &ArchiveLosslessnessReport) -> String {
    let mut lines: Vec<String> = Vec::new();
    for team in &report.reports {
        lines.push(format!("{} [{}]", team.team_slug, team.provider));
        if !team.covered {
            lines.push(format!(
                "  not covered: {}",
                team.skipped.as_deref().unwrap_or("")
            ));
            lines.push(String::new());
            continue;
        }
        let verdict = if team.lossless() {
            "vendor snapshots are redundant"
        } else if team.sound() {
            "vendor snapshots still hold content the archive does not"
        } else {
            "AUDIT FAILED -- the archive's account of itself does not hold"
        };
        lines.push(format!(
            "  {} rows in {} files, {}: {verdict}",
            commafy(team.vendor_rows),
            team.vendor_files,
            mib(team.vendor_bytes)
        ));
        if team.absent_rows != 0 {
            lines.push(format!(
                "  content absent from the archive: {} rows, {} ({:.1}% of the corpus)",
                commafy(team.absent_rows),
                mib(team.absent_bytes),
                100.0 * team.absent_bytes as f64 / team.vendor_bytes.max(1) as f64
            ));
        }
        if team.partial_rows != 0 {
            lines.push(format!(
                "  content only partly held: {} rows, up to {} ({:.1}% of the corpus; \
                 whole-row attribution, so an upper bound)",
                commafy(team.partial_rows),
                mib(team.partial_bytes),
                100.0 * team.partial_bytes as f64 / team.vendor_bytes.max(1) as f64
            ));
        }
        for tally in &team.tallies {
            let marker = if tally.blocks_deletion { "!" } else { " " };
            let mut line = format!(
                "  {marker} {:<26} {:<9} {:>9} rows {:>12}",
                tally.name,
                tally.disposition,
                commafy(tally.rows),
                mib(tally.vendor_bytes)
            );
            if tally.unverified != 0 {
                line.push_str(&format!("  {} UNVERIFIED", commafy(tally.unverified)));
            }
            lines.push(line);
            if tally.blocks_deletion {
                lines.push(format!("      {}", tally.reason));
            }
        }
        for problem in &team.source_problems {
            lines.push(format!("  vendor snapshots: {problem}"));
        }
        for problem in &team.payload_problems {
            lines.push(format!("  payload store: {problem}"));
        }
        for finding in &team.unaccounted {
            lines.push(format!(
                "  UNACCOUNTED {}:{} ({}/{}): {}",
                finding.source_path,
                finding.line,
                opt_text(&finding.top_type),
                opt_text(&finding.payload_type),
                finding.detail
            ));
        }
        if team.unaccounted_rows > team.unaccounted.len() as i64 {
            lines.push(format!(
                "  ... {} further unaccounted rows",
                commafy(team.unaccounted_rows - team.unaccounted.len() as i64)
            ));
        }
        for finding in &team.unverified {
            lines.push(format!(
                "  UNVERIFIED {}:{} ({}/{}): {}",
                finding.source_path,
                finding.line,
                opt_text(&finding.top_type),
                opt_text(&finding.payload_type),
                finding.detail
            ));
        }
        if team.unverified_rows > team.unverified.len() as i64 {
            lines.push(format!(
                "  ... {} further unverified rows",
                commafy(team.unverified_rows - team.unverified.len() as i64)
            ));
        }
        lines.push(String::new());
    }
    lines.push(format!(
        "sound: {}; lossless: {}",
        if report.sound() { "yes" } else { "NO" },
        if report.lossless() { "yes" } else { "no" }
    ));
    let mut out = lines.join("\n");
    out.push('\n');
    out
}
