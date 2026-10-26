//! Discoverable command-line interface for persistent named agents.
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Args, CommandFactory, Parser, Subcommand};
use serde::Serialize;
use serde_json::{json, Value};

use crate::agent::{AgentError, DrainOptions, QueueOutcome};
use crate::client::HerdrClient;
use crate::subagents::{
    environment_entries, stop_reason, AdoptOptions, CloudLaunch, CloudTools, ManageAction,
    ManagedAgents, RecoveryAction, SlotLaunch, StartOptions, StopContext, StopFailure, StopOptions,
};

const MAX_CHAT_PUBLISH_BYTES: usize = 30_000;

/// Validate a `--term-backend` value at parse time.
fn parse_term_backend(raw: &str) -> std::result::Result<String, String> {
    match raw {
        "herdr" | "tmux" => Ok(raw.to_owned()),
        other => Err(format!(
            "unknown terminal backend {other:?}; expected herdr or tmux"
        )),
    }
}

#[derive(Parser)]
#[command(
    name = "agentctl",
    version,
    about = "Manage persistent coding agents that you can message and inspect",
    long_about = "Manage named coding-agent sessions with durable prompt delivery and direct human access.\nThe Rust implementation controls interactive Codex, Claude, and Muse sessions through Herdr and can run a durable event-driven chat bridge through installed subscription plugins.",
    after_help = "Examples:\n  agentctl quickstart\n  agentctl start reviewer --cwd .\n  agentctl adopt reviewer --pane w1:p2 --workspace project --cwd /work/project --harness codex\n  agentctl send reviewer 'Review the current changes'\n  agentctl chat status --bridge-state ~/.local/state/agentctl/chat\n  agentctl pause reviewer\n  agentctl attach reviewer\n  agentctl resume reviewer\n  agentctl stop reviewer"
)]
struct Cli {
    /// Registry directory containing agent records and durable queues
    #[arg(
        long,
        visible_alias = "state",
        global = true,
        default_value = ".agentctl",
        value_name = "DIR"
    )]
    registry: PathBuf,
    /// Herdr executable name on PATH or an explicit path (interactive adapter only)
    #[arg(long, global = true, default_value = "herdr", value_name = "PATH")]
    herdr_bin: PathBuf,
    /// Terminal multiplexer backend: herdr (default) or tmux. Default:
    /// $AGENTCTL_TERM_BACKEND when set and nonempty, else the profiles.json
    /// `term_backend` key, else herdr when its executable is available, else tmux.
    /// Both backends run the full agent lifecycle and the durable direct-pane
    /// transport; the chat, inbox, restore, and watch services remain Herdr-only
    #[arg(long, global = true, value_parser = parse_term_backend, value_name = "BACKEND")]
    term_backend: Option<String>,
    /// tmux executable name on PATH or an explicit path (tmux backend only)
    #[arg(long, global = true, default_value = "tmux", value_name = "PATH")]
    tmux_bin: PathBuf,
    /// tmux server socket name passed as -L (tmux backend only); default: the default tmux server
    #[arg(long, global = true, value_name = "NAME")]
    tmux_socket: Option<String>,
    /// agentcloudctl executable name on PATH or an explicit path (agentcloud agents only)
    #[arg(
        long,
        global = true,
        default_value = "agentcloudctl",
        value_name = "PATH"
    )]
    agentcloudctl_bin: PathBuf,
    /// agentterm executable run in an agentcloud agent's Herdr tab; name on PATH or explicit path
    #[arg(long, global = true, default_value = "agentterm", value_name = "PATH")]
    agentterm_bin: PathBuf,
    /// Agentcloud orchestrator endpoint recorded for NEW agentcloud agents and passed as --ws-url to both agentcloudctl and agentterm; default: $AGENTCLOUD_ORCHESTRATOR_URL when set and nonempty, else agentcloudctl's documented production endpoint wss://mm.internalmeta.com/ws/chat. Later commands use the endpoint recorded for the agent and refuse a different explicit value; a record from before endpoints were recorded gets this resolved endpoint saved on its first send or stop
    #[arg(long, global = true, value_name = "URL")]
    agentcloud_url: Option<String>,
    /// Agentcloud session this caller runs in; input to another agentcloud agent is then an attributed `agentcloudctl send-message` from it. An empty value selects human attribution (`agentcloudctl send`), which is refused when this process runs inside a different agentcloud session than the recipient's. Default: $AGENTCLOUD_SESSION_ID when set and nonempty
    #[arg(long, global = true, value_name = "ID")]
    from_session: Option<String>,
    /// Print the installed operator reference and exit
    #[arg(long)]
    userguide: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show installed runtime adapters and optional services as JSON
    #[command(after_help = "Example: agentctl capabilities")]
    Capabilities,
    /// Launch an interactive agent, or an agentcloud session viewed through agentterm, in its own Herdr tab
    #[command(
        after_help = "Examples:\n  agentctl start reviewer --cwd . --harness codex --brief 'Review the changes'\n  agentctl start sub-cloud-worker --harness agentcloud --cloud-harness claude-code \\\n    --provision --envspec ENVSPEC --brief 'Run hostname, then reply DONE.'\n  agentctl start sub-cloud-worker --cwd . --profile cloud-worker --brief 'Investigate the failure'\n\nWith --harness agentcloud, agentctl runs `agentcloudctl create` (title = NAME, the brief is its\ndurable --prompt), records the printed session ID, then runs `agentterm -s SESSION_ID` in the\nnew tab. Both receive the same --ws-url: --agentcloud-url, else $AGENTCLOUD_ORCHESTRATOR_URL, else\nagentcloudctl's documented production endpoint; it is recorded for every later command. --model and --reasoning-effort map to create --model and --effort; --harness-arg adds\nliteral long create options written as --option=value. `agentcloudctl create` is bounded at\n300 seconds; --startup-timeout bounds the wait for agentterm to become the tab's foreground\nprocess. --env applies to the tab only, never to agentcloudctl."
    )]
    Start(Box<Start>),
    /// List safe metadata for ignored private launch profiles
    #[command(after_help = "Example: agentctl profiles --cwd /work/project")]
    Profiles(Profiles),
    /// Install the bundled agentctl harness skill
    Skill(Skill),
    /// Register an existing agent in a Herdr or tmux pane without taking ownership of its runtime; Muse requires a pinned foreground process
    #[command(
        after_help = "Examples:\n  agentctl adopt reviewer --pane w1:p2 --workspace project --cwd /work/project --harness codex\n  agentctl --term-backend tmux adopt reviewer --pane %2 --workspace project --cwd /work/project --harness claude"
    )]
    Adopt(Adopt),
    /// Show, set, or clear this registry's designated coordinator: the root of the control tree, the agent watch-agents and restore notify when no nearer manager is recorded
    #[command(
        after_help = "Examples:\n  agentctl coordinator set coordinator\n  agentctl coordinator show\n  agentctl coordinator clear\n\nThe designation is REGISTRY/coordinator.json (schema agentctl-coordinator/v1: name, token, set_at,\nset_by). It names one agent generation by its token, so it follows the agent through rename and revive;\nshow reports coordinator-stopped once that agent is stopped, and harness_pid, the pinned harness\nprocess a slot manager can take as the coordinator's process. adopt --as-coordinator and\nattach-self --as-coordinator set it in the same step. watch-agents and restore use it when no\nexplicit coordinator is given."
    )]
    Coordinator(Coordinator),
    /// Change which agent controls a registered agent: the calling agent (proved by process ancestry), a named agent, or none
    #[command(
        after_help = "Examples:\n  agentctl manage worker-1\n  agentctl manage worker-1 --by lieutenant\n  agentctl manage worker-1 --clear\n\nmanaged_by decides who controls an agent: whom watch-agents tells about it, and where it sits in\n`agentctl tree`. start records the calling agent as both created_by (provenance, never changed) and\nmanaged_by; adopt records the adopting agent as managed_by. With neither --by nor --clear, the\ncalling agent takes over: it must be a registered agent of this registry whose pinned harness is an\nancestor of this process. A manager that would make a cycle is refused. An agent with no manager\nanswers to the designated coordinator."
    )]
    Manage(Manage),
    /// Print the control tree: the designated coordinator at the root and every agent under the agent that manages it, at any depth
    #[command(
        after_help = "Examples:\n  agentctl tree\n  agentctl tree --json\n\nAn agent hangs under the registered agent its managed_by names; one with no manager, a stopped\nmanager, or a manager in another registry hangs under the designated coordinator. created_by is shown\nwhere it differs from managed_by. --json adds tokens, edges and each agent's pinned harness process."
    )]
    Tree(Tree),
    /// Move a running owned native Herdr agent into an existing configured workspace; adopted, custom-pane, and multi-pane tabs are refused
    #[command(after_help = "Example: agentctl move reviewer")]
    Move(Named),
    /// Rename a running native or adopted agent: registry entry, Herdr agent name, and tab label together. Use it when an agent's warm context is still useful but its purpose changed; prefer a new agent otherwise. An interrupted rename refuses both names until the same command is rerun
    #[command(after_help = "Example: agentctl rename reviewer release-reviewer")]
    Rename(Rename),
    /// Pin the terminal and foreground harness process of an agent whose record pins no harness process, after checking that its pane runs the intended agent. Input to an agent with no pinned harness process or observed native session is refused. For a Muse pane or root slot relay, whose process is pinned at launch, only record that process's launch spec
    #[command(after_help = "Example: agentctl anchor reviewer")]
    Anchor(Anchor),
    /// Mark a launch_failed record running when its harness is in fact live: Herdr reports the launched harness in the recorded pane, tab, workspace and cwd, the Herdr agent name still points there, and the harness is the pane's only foreground process. Pins that process and terminal; never touches the tab, pane, or process. Use it when a readiness wait timed out on a harness that runs fine
    #[command(after_help = "Example: agentctl promote reviewer")]
    Promote(Promote),
    /// Bring the coordinator team back after the host restarts: remount slot images, wait for Herdr, revive the coordinator and then every dead agent from its launch spec, managers before the agents they manage, register the coordinator's new pane in other registries, start the configured units, and queue one recovery notice. Idempotent; writes a JSON report for the boot
    #[command(
        after_help = "Examples:\n  agentctl --registry ~/project/.agentctl restore --dry-run\n  agentctl --registry ~/project/.agentctl restore --boot\n\nConfiguration is REGISTRY/restore.json (schema agentctl-restore/v1): coordinator (default: the\nregistry's designated coordinator; when both are set and disagree, restore.json wins and the report\nsays so under designation_problem), also_registered\n([{registry, name}]), units, exclude (names to leave alone), startup_delay_seconds (default 20), herdr_wait_seconds (default 300),\nreport_dir (default $XDG_STATE_HOME/agentctl/restore), herdr_unit, and timers (used by watch).\nThe slot manager is AGENTCTL_WRKSLOTS_BIN, then profiles.json \"wrkslots\", then PATH; restore runs\n`wrkslots image status` once, before any agent starts, so image-backed slots are mounted.\nA completed restore for this boot makes a rerun exit 0 at once unless --force. Exit 0 when the\nrestore completed with no refusal, 75 otherwise; every step and refusal is in the report."
    )]
    Restore(Restore),
    /// Write the user units that restore the team at boot: a persistent herdr-server.service, the agent-restore.service boot hook, agent-watch.service, and a drop-in that orders the chat bridge after Herdr and restarts it on failure (start limits stay with the bridge's own policy). Never enables, starts, or reloads anything
    #[command(
        after_help = "Example:\n  agentctl --registry ~/project/.agentctl --herdr-bin ~/bin/herdr install-units --dest ~/.config/systemd/user --bridge-unit chat-bridge.service --dry-run\n\nPaths are rendered absolute: --agentctl (default: this executable), --herdr-bin (looked up on PATH\nwhen it is a bare name), and --registry. A file that exists with different contents is refused\nunless --force. The output lists the files and the systemctl commands an operator runs next."
    )]
    InstallUnits(InstallUnits),
    /// Check and repair, every interval: the Herdr server (start herdr_unit), each configured unit (reset-failed and start), each configured timer (restart it; start its service when it still never fires; report, never repair, a service that failed on its last run), slot images (wrkslots image status, on the first pass and every tenth), this boot's restore, and the coordinator's harness. A unit or timer that systemctl could not check, even after one retry 5 seconds later, is a warning: never repaired and never alerted on. Writes health.json and tells the coordinator, at most hourly per problem, about anything it could not repair
    #[command(
        after_help = "Examples:\n  agentctl --registry ~/project/.agentctl watch --once\n  agentctl --registry ~/project/.agentctl watch --interval 60\n\nConfiguration is REGISTRY/restore.json (herdr_unit, units, timers [{unit, max_age_seconds}],\ncoordinator). health.json goes to --state-dir (default $XDG_STATE_HOME/agentctl/watch). With --once,\nexit 0 when every check passed and 1 otherwise. Without it the watch runs until stopped."
    )]
    Watch(Watch),
    /// Watch every registered agent and tell its manager, else the coordinator, when one finished its turn with a verdict or needs a human: at a harness approval or permission prompt, idle without a verdict, stalled with no new output, or dead. One notice per episode, queued durably for its recipient; never answers a prompt and never types into any other agent
    #[command(
        after_help = "Examples:\n  agentctl --registry ~/project/.agentctl watch-agents --coordinator coordinator --once --check-only\n  agentctl --registry ~/project/.agentctl watch-agents --coordinator coordinator --approval-minutes 5 --idle-minutes 5 --stall-minutes 30\n\nEach pass lists the registry, reads every running or starting agent except the coordinator (paused, mid-move, and agentcloud\nagents are skipped; an unreadable record is watched as unreachable), and classifies it. A condition that holds past its\nthreshold is reported once, as one message queued through the same durable path as `agentctl send`, so a busy recipient\nloses nothing. The recipient is the nearest reachable agent up the agent's managed_by chain, else the coordinator\n(`agentctl tree` shows the chain). A verdict is a prose line (not tool output) of the agent's last reply (from its first block after the last user turn\non screen) that starts with one of --done-marker once any reply bullet is stripped (--done-marker replaces the DONE:/BLOCKED:\ndefaults). An idle agent with a verdict is reported as child-done after --child-done-seconds (default 0: the pass that sees it),\nin a message of its own that names the verdict line, once per turn, never held by --min-notice-interval; when the registry's\nlanding queue (`agentctl hold`) has agents waiting on it, the same message says who is next. An agent that was never sent\nwork, or still has prompts queued, is never reported idle, and a Claude Code view scrolled up (`N new messages (ctrl+End) ↓`\nabove the input box) is never judged stalled, idle, or finished until it is back at the bottom. Episode memory lives in\n--state-dir (default $XDG_STATE_HOME/agentctl/agent-watch); a message's queue ID is derived from its notices, so a restart\nrecognises a message that is already queued instead of sending it twice, and one watcher runs per state directory. With\n--once, exit 0 only when the team could be probed, no episode other than child-done is reported or due, and no view is\nscrolled up, 1 otherwise.\nWithout it the watch runs until stopped; see the sample user unit in `agentctl userguide`."
    )]
    WatchAgents(WatchAgents),
    /// Record that an agent lands after another (the landing queue), list the queue, or drop a hold. `watch-agents` reads it: when OTHER finishes its turn with a verdict that does not start with BLOCKED, OTHER's child-done notice to the coordinator says NAME is next, and once that notice is queued the hold is marked released. Nothing is ever sent to NAME
    #[command(
        after_help = "Examples:\n  agentctl --registry ~/project/.agentctl hold cl-ingest --after cl-dedupe --note \"gate green, ready to push\"\n  agentctl --registry ~/project/.agentctl hold --list\n  agentctl --registry ~/project/.agentctl hold cl-ingest --release\n\nThe queue is REGISTRY/landing-queue.json (schema agentctl-landing-queue/v1). One hold per NAME: a new hold replaces the old.\nA chain (B after A, C after B) advances one step per finish. A released hold stays listed, with released_by and\nreleased_at, for a day. Prints the queue as JSON; --release exits 1 when NAME had no hold."
    )]
    Hold(Hold),
    /// Compare every registry record with live Herdr state and report each mismatch. Read-only unless --repair-labels. Exit 0 when clean, 1 with findings
    #[command(after_help = "Example: agentctl doctor")]
    Doctor(Doctor),
    /// Stop an owned runtime, or safely unregister an adopted one, and archive state
    #[command(
        after_help = "Examples:\n  agentctl stop reviewer\n  agentctl stop sub-cloud-worker\n  agentctl stop reviewer --retire-dead-adoption --expected-token TOKEN --expected-record-sha256 SHA256\n\nFor an agentcloud agent, stop runs `agentcloudctl halt` (the open run is interrupted and no new\nrun starts), closes the recorded agentterm pane, runs `agentcloudctl archive`, and archives the\nregistry record. A halt failure changes nothing locally. stop does not release the node\nreservation; the orchestrator releases it on its own schedule after the halted session goes\nidle (check `agentcloudctl inspect -s SESSION_ID`).\n\nA headless turn-runner worker is retired only when every process its runtime recorded has\nprovably exited: stop --expected-token then archives the record with its runtime home and leaves\nthe tmux window or Herdr tab in place. A live or unprovable runner exits 69 and changes nothing."
    )]
    Stop(Stop),
    /// Relaunch a dead local agent by replaying its recorded launch spec and conversation, preserving its old queue in the archive
    #[command(
        after_help = "Examples:\n  agentctl revive reviewer\n  agentctl revive --all --dry-run\n  agentctl revive reviewer --expected-token TOKEN\n\nA live or unverifiable harness is never relaunched. Recovery opens a fresh tab, verifies the\nresumed runtime, archives the complete old generation, then closes only its proved stale pane.\nThe launch spec (`launch_spec` in status) is what ran: start writes it, adopt and anchor read it\nfrom the pinned process's command line. A record without one is rebuilt from its launch policy\nand profile. An adopted agent needs a spec, and its old pane is left open; the replacement is\nowned. An interrupted revive reserves the name until the same command finishes publication or\ncleanup. Headless workers and remote agentcloud sessions are refused."
    )]
    Revive(Revive),
    /// List registered agents with live status or an explicit probe error
    #[command(after_help = "Example: agentctl list --registry .agentctl")]
    List,
    /// Show durable metadata, queue state, and the live runtime probe
    #[command(after_help = "Example: agentctl status reviewer")]
    Status(Named),
    /// Persist and deliver a prompt when the agent is ready
    #[command(
        after_help = "Examples:\n  agentctl send reviewer 'Review the diff'\n  agentctl send reviewer --file task.txt --message-id review-1\n\nFor an agentcloud agent the prompt is journaled by `agentcloudctl send --end-of-turn` (a leading\n/goal is sent without --end-of-turn), up to 102400 bytes. When --from-session (default:\n$AGENTCLOUD_SESSION_ID) names a different agentcloud session, agentcloudctl refuses that send,\nso the prompt goes as an attributed `agentcloudctl send-message` from that session instead; it\narrives as an ordinary input and a /goal is not interpreted. An explicit\nhuman send (--from-session '') from inside a different agentcloud session is refused, never\ndisguised. --message-id becomes the idempotency\nkey, so repeating it journals nothing new; without it agentctl generates and prints one. The\ndelivery timeouts and --max-attempts do not apply."
    )]
    Send(Send),
    /// Deliver queued prompts that are known not to have been submitted
    #[command(
        after_help = "Examples:\n  agentctl drain reviewer --ready-timeout 60\n  agentctl drain reviewer --deadline 40 --lock-timeout 10 --max-items 1\n\nThe drain types inbox/ and inbox.v2/ together, oldest first. Just before typing each prompt it\nchecks for a tombstone (agentctl cancel) and for an expired --ttl, and sets such a prompt aside in\ncancelled/ instead. Exit 0 when every examined prompt was confirmed, 75 when prompts remain pending\n(not ready, a bound reached, or a lock timeout), 76 when one was quarantined as possibly submitted."
    )]
    Drain(Drain),
    /// Ask every drain not to type a queued prompt: write its tombstone without waiting for any lock, and report where the prompt is
    #[command(
        after_help = "Examples:\n  agentctl cancel reviewer review-1\n  agentctl cancel reviewer hold-3 --reason superseded\n\nThe tombstone is QUEUE/cancel/MESSAGE_ID.json. A drain checks it after taking the prompt and before\ntyping, so a prompt reported pending is never typed: the drain moves it to cancelled/. outcome is\npending, unknown (no such prompt yet; one that arrives later is set aside too), inflight (a drain\nhas taken it and may be typing it), too_late (typed or quarantined), or already_cancelled. Once a\nqueue has a tombstone, the other edition refuses to drain it."
    )]
    Cancel(Cancel),
    /// Inspect blocking questions; reveal collapsed forms without selecting or submitting
    #[command(after_help = "Example: agentctl prompts reviewer")]
    Prompts(Named),
    /// Answer a recognized TUI question, verifying dismissal; never retry uncertain answers
    #[command(
        after_help = "Examples:\n  agentctl answer reviewer --option 2\n  agentctl answer reviewer --text 'Use the existing approach'"
    )]
    Answer(Answer),
    /// Read visible terminal output and save a bounded snapshot
    #[command(
        after_help = "Examples:\n  agentctl read reviewer --lines 100\n  agentctl read sub-cloud-worker --output last\n\nFor an agentcloud agent, --output last prints the last settled run's final reply from\n`agentcloudctl output` (unbounded by --lines); tail and all read the agentterm tab."
    )]
    Read(Read),
    /// Wait until the agent is ready for input; this does not prove goal completion
    #[command(
        after_help = "Example: agentctl wait reviewer --timeout 60\n\nFor an agentcloud agent this is `agentcloudctl wait --until settle`: success means the current or\nlast run completed (or none has run); a failed or interrupted run exits 75 with a redirect."
    )]
    Wait(Wait),
    /// Inspect a goal or set an ongoing objective (native /goal for Codex)
    #[command(
        after_help = "Examples:\n  agentctl goal reviewer\n  agentctl goal reviewer 'Complete the review and report blockers'"
    )]
    Goal(Goal),
    /// Bind an explicitly known native conversation ID to a verified agent pane
    #[command(after_help = "Example: agentctl bind-session reviewer THREAD_ID")]
    BindSession(BindSession),
    /// Read, set, or clear a native Codex goal through the app-server protocol, without resuming or prompting its thread
    #[command(after_help = crate::codex_goal::command::AFTER_HELP)]
    CodexGoal(crate::codex_goal::command::CodexGoalArgs),
    /// Verify and focus the agent pane for human interaction; does not pause automation
    #[command(after_help = "Example: agentctl pause reviewer && agentctl attach reviewer")]
    Attach(Named),
    /// Show the agentcloud session this process runs in through its viewer in a Herdr tab, and register it so it is listed beside the agents it starts
    #[command(
        after_help = "Examples:\n  agentctl attach-self coordinator\n  agentctl attach-self coordinator --cwd . --workspace-id w2\n\nThe session is $AGENTCLOUD_SESSION_ID; without it attach-self is refused, because a session\nin a local terminal cannot be moved into Herdr. Nothing changes in agentcloud: the tab runs\n`agentterm -s SESSION_ID` with the same endpoint resolution as start. The tab goes to\n--workspace-id, else the configured project workspace, else HERDR_WORKSPACE_ID, else the\nworkspace labelled with the --cwd directory's base name, which is created when absent; agents this session starts later\nwithout an explicit workspace open in the same workspace. stop NAME closes the tab and archives\nthe record but never halts or archives the session, which is the caller itself."
    )]
    AttachSelf(AttachSelf),
    /// Pause automation input without suspending the harness or its running work
    #[command(after_help = "Example: agentctl pause reviewer")]
    Pause(Named),
    /// Reset an idle headless turn-runner worker's conversation context
    #[command(after_help = "Example: agentctl reset worker")]
    Reset(Named),
    /// Re-enable automation input; queued messages still require drain or send
    #[command(after_help = "Example: agentctl resume reviewer && agentctl drain reviewer")]
    Resume(Named),
    /// Serve MCP stdio access to the same named sessions and operations as this CLI
    #[command(after_help = "Example: agentctl mcp")]
    Mcp,
    /// Initialize, inspect, recover, or run a durable chat bridge
    #[command(
        after_help = "Example: agentctl chat status --bridge-state ~/.local/state/agentctl/chat"
    )]
    Chat(Chat),
    /// Queue notices about workers for a coordinator and deliver them as one prioritized batch
    #[command(
        after_help = "Examples:\n  agentctl inbox quickstart\n  agentctl inbox post --to coord --from reviewer --kind idle --text 'Review done'\n  agentctl inbox deliver --to coord --via print"
    )]
    Inbox(crate::inbox::InboxArgs),
    /// Print a short, runnable introduction to starting and controlling a worker
    Quickstart,
    /// Print the operator reference, including dependencies and delivery guarantees
    Userguide,
}

#[derive(Args)]
struct Named {
    /// Registered agent name (1-32 lowercase letters, digits, and hyphens)
    name: String,
}

#[derive(Args)]
struct Coordinator {
    #[command(subcommand)]
    command: CoordinatorCommand,
}

#[derive(Subcommand)]
enum CoordinatorCommand {
    /// Designate a registered agent as this registry's coordinator
    Set(Named),
    /// Print the designation and the state of the agent it names, as JSON; exit 1 when none is designated
    Show,
    /// Remove the designation
    Clear,
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("manager").args(["by", "clear"])))]
struct Manage {
    #[command(flatten)]
    agent: Named,
    /// Make this registered agent the manager (default: the calling agent)
    #[arg(long, value_name = "MANAGER")]
    by: Option<String>,
    /// Remove the manager; the agent then answers to the designated coordinator
    #[arg(long)]
    clear: bool,
}

#[derive(Args)]
struct Tree {
    /// Print the tree as JSON instead of indented text
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("selection").required(true).args(["option", "text"])))]
struct Answer {
    #[command(flatten)]
    agent: Named,
    /// One-based option number printed by prompts
    #[arg(long, value_name = "N")]
    option: Option<usize>,
    /// Literal single-line answer for a supported empty or identical-draft free-text form
    #[arg(long, value_name = "TEXT")]
    text: Option<String>,
}

#[derive(Args)]
struct Restore {
    /// Mark the report as the boot hook's run (agent-restore.service passes it)
    #[arg(long)]
    boot: bool,
    /// Run even when this boot's report already says the restore completed
    #[arg(long)]
    force: bool,
    /// Report what would be done without reviving, promoting, registering, starting units, notifying, or writing a report
    #[arg(long)]
    dry_run: bool,
    /// Startup-readiness deadline for each revived harness in seconds, greater than 0 and at most 300 (default: 30)
    #[arg(long, default_value = "30", value_parser = startup_seconds, value_name = "SECONDS")]
    startup_timeout: f64,
}

#[derive(Args)]
struct InstallUnits {
    /// Directory to write the units into, normally ~/.config/systemd/user (required)
    #[arg(long, value_name = "DIR")]
    dest: PathBuf,
    /// The agentctl executable the units run (default: this executable)
    #[arg(long, value_name = "PATH")]
    agentctl: Option<PathBuf>,
    /// The chat bridge unit to order after the Herdr server; writes UNIT.d/zz-agentctl-after-herdr.conf, named to sort after numbered drop-ins
    #[arg(long, value_name = "UNIT")]
    bridge_unit: Option<String>,
    /// List what would be written without writing
    #[arg(long)]
    dry_run: bool,
    /// Replace existing files whose contents differ
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct Watch {
    /// Seconds between passes, from 10 to 3600 (default: 60)
    #[arg(long, default_value = "60", value_parser = clap::value_parser!(u64).range(10..=3600), value_name = "SECONDS")]
    interval: u64,
    /// Run one pass, print it as JSON, and exit
    #[arg(long)]
    once: bool,
    /// Report only: start, restart, reset-failed, and remount nothing, and send no alert
    #[arg(long)]
    check_only: bool,
    /// Directory for health.json (default: $XDG_STATE_HOME/agentctl/watch)
    #[arg(long, value_name = "DIR")]
    state_dir: Option<PathBuf>,
}

#[derive(Args)]
struct WatchAgents {
    /// Registered name of the coordinator that receives notices about agents with no reachable manager, and is not itself watched (default: the registry's designated coordinator, else coordinator). A notice about an agent goes to the nearest reachable agent up its managed_by chain instead
    #[arg(long, value_name = "NAME")]
    coordinator: Option<String>,
    /// Seconds between passes, from 10 to 3600 (default: 60)
    #[arg(long, default_value = "60", value_parser = clap::value_parser!(u64).range(10..=3600), value_name = "SECONDS")]
    interval: u64,
    /// Run one pass, print it as JSON, and exit
    #[arg(long)]
    once: bool,
    /// Report only: send no notice to the coordinator
    #[arg(long)]
    check_only: bool,
    /// Minutes an approval or permission prompt may hold before it is reported, from 1 to 1440 (default: 5)
    #[arg(long, default_value = "5", value_parser = watch_minutes, value_name = "MINUTES")]
    approval_minutes: u64,
    /// Minutes an agent may sit idle without a verdict, with an unchanged screen, before it is reported, from 1 to 1440 (default: 5); an idle agent whose screen shows work running (Claude Code's `esc to interrupt`, or a count of background shells or agents) is held to --stall-minutes instead
    #[arg(long, default_value = "5", value_parser = watch_minutes, value_name = "MINUTES")]
    idle_minutes: u64,
    /// Seconds an agent may sit idle with a verdict in its last reply, with an unchanged screen, before it is reported child-done, from 0 to 3600 (default: 0, the pass that first sees it); a child-done notice is its own message, sent at once, never held by --min-notice-interval
    #[arg(long, default_value = "0", value_parser = clap::value_parser!(u64).range(0..=3600), value_name = "SECONDS")]
    child_done_seconds: u64,
    /// Minutes a working agent, or an idle one whose screen shows work running, may show no new output before it is reported stalled, from 1 to 1440 (default: 30)
    #[arg(long, default_value = "30", value_parser = watch_minutes, value_name = "MINUTES")]
    stall_minutes: u64,
    /// Minutes a running record's harness may be unreachable before it is reported dead, from 1 to 1440 (default: 5)
    #[arg(long, default_value = "5", value_parser = watch_minutes, value_name = "MINUTES")]
    dead_minutes: u64,
    /// Line prefix that counts as a verdict; repeatable, and replaces the defaults (default: DONE: and BLOCKED:)
    #[arg(long = "done-marker", value_name = "PREFIX")]
    done_markers: Vec<String>,
    /// Minimum seconds between two batched messages to the coordinator; notices other than child-done batch and wait, from 0 to 86400 (default: 300)
    #[arg(long, default_value = "300", value_parser = clap::value_parser!(u64).range(0..=86400), value_name = "SECONDS")]
    min_notice_interval: u64,
    /// Directory for agent-watch-state.json and agent-watch.json (default: $XDG_STATE_HOME/agentctl/agent-watch)
    #[arg(long, value_name = "DIR")]
    state_dir: Option<PathBuf>,
    /// Minutes a sender may wait for an agent's lifecycle lock before the coordinator is told, from 0 to 1440 (default: 10; 0 turns the check off). Every managed send holds that lock until its delivery is confirmed, so messages to a busy agent wait in turn; the notice names the holder and the waiters, is sent once per jam, and goes ahead of ordinary sends
    #[arg(long, default_value = "10", value_parser = clap::value_parser!(u64).range(0..=1440), value_name = "MINUTES")]
    delivery_wait_minutes: u64,
}

#[derive(Args)]
struct Hold {
    /// Agent that waits to land (1-32 lowercase letters, digits, and hyphens); required unless --list
    #[arg(value_name = "NAME", required_unless_present = "list")]
    name: Option<String>,
    /// Agent whose finish releases NAME; required to record a hold
    #[arg(long, value_name = "OTHER", required_unless_present_any = ["list", "release"], conflicts_with_all = ["list", "release"])]
    after: Option<String>,
    /// Free text repeated in the release notice, such as what NAME is ready to land (default: empty)
    #[arg(long, value_name = "TEXT", default_value = "", requires = "after")]
    note: String,
    /// Drop NAME's hold, released or not, instead of recording one
    #[arg(long, conflicts_with = "list")]
    release: bool,
    /// Print the landing queue and change nothing
    #[arg(long, conflicts_with = "name")]
    list: bool,
}

#[derive(Args)]
struct Promote {
    /// Registered agent name (1-32 lowercase letters, digits, and hyphens)
    name: String,
    /// Require this exact record generation token
    #[arg(long, value_name = "TOKEN")]
    expected_token: Option<String>,
}

#[derive(Args)]
struct Revive {
    /// Registered name to recover; required unless --all
    #[arg(
        value_name = "NAME",
        required_unless_present = "all",
        conflicts_with = "all"
    )]
    name: Option<String>,
    /// Inspect every registered generation and continue through independent recovery refusals
    #[arg(long, conflicts_with_all = ["name", "expected_token"])]
    all: bool,
    /// Print safe recovery plans without changing records, tabs, queues, or lock files
    #[arg(long)]
    dry_run: bool,
    /// Require this exact old generation token; applies only to a named revive
    #[arg(long, requires = "name", value_name = "TOKEN")]
    expected_token: Option<String>,
    /// Startup-readiness deadline in seconds, greater than 0 and at most 300 (default: 30)
    #[arg(long, default_value = "30", value_parser = startup_seconds, value_name = "SECONDS")]
    startup_timeout: f64,
}

#[derive(Args)]
struct Rename {
    #[command(flatten)]
    agent: Named,
    /// New registered name (lowercase letters, digits, hyphens)
    #[arg(value_name = "NEW")]
    new_name: String,
}

#[derive(Args)]
struct Anchor {
    #[command(flatten)]
    agent: Named,
    /// Replace anchors that no longer match (the pane may hold another program; inspect it first)
    #[arg(long)]
    replace: bool,
    /// Native conversation id to record in the launch spec when Herdr reports none; a Claude agent anchoring itself can pass "$CLAUDE_CODE_SESSION_ID". Refused if it contradicts Herdr's report. Default: Herdr's report, then the conversation the harness command line selects
    #[arg(long = "session-id", value_name = "ID")]
    session_id: Option<String>,
}

#[derive(Args)]
struct Doctor {
    /// Restore the tab label and Herdr agent name of owned agents whose every other anchor still matches
    #[arg(long)]
    repair_labels: bool,
}

#[derive(Args)]
struct Stop {
    #[command(flatten)]
    agent: Named,
    /// Require this exact current registry generation; mandatory for dead managed, adopted, or headless turn-runner recovery
    #[arg(long, value_name = "TOKEN")]
    expected_token: Option<String>,
    /// Recover an identity-less dead adopted row without changing its pane; requires --expected-token and --expected-record-sha256
    #[arg(long)]
    recover_legacy_adoption: bool,
    /// Archive a running adopted record whose pinned harness is dead, without querying Herdr; requires --expected-token and --expected-record-sha256
    #[arg(long, conflicts_with = "recover_legacy_adoption")]
    retire_dead_adoption: bool,
    /// Exact lowercase 64-hex SHA-256 of raw agent.json bytes; valid only with an adopted-record recovery flag
    #[arg(long, value_name = "SHA256")]
    expected_record_sha256: Option<String>,
    /// Agentcloud agents only: close the tab and archive the record WITHOUT halting or archiving
    /// the agentcloud session (use when the session no longer exists or was retired elsewhere)
    #[arg(long)]
    skip_cloud_halt: bool,
}

#[derive(Args)]
struct Delivery {
    /// Seconds to wait for a submit-safe state (0 probes immediately)
    #[arg(long, default_value = "900", value_parser = seconds)]
    ready_timeout: f64,
    /// Positive seconds to wait for evidence that a submitted prompt started work
    #[arg(long, default_value = "30", value_parser = positive_seconds)]
    working_timeout: f64,
    /// Maximum known-safe submission attempts per message
    #[arg(long, default_value = "3", value_parser = clap::value_parser!(u64).range(1..=1_000_000))]
    max_attempts: u64,
    /// Also type into a Claude Code pane that is working (native, or behind a root slot relay), and let Claude Code queue the prompt behind its current turn (default: off, which waits for idle or done up to --ready-timeout). The composer must be empty first and the submission verified; a prompt Claude Code never accepts is taken back out and stays pending. Codex and Muse ordinary follow-ups are delivered while working by default (Muse: pasted into its empty editor, where Enter steers the running turn, proved by the new user turn; a prompt Muse never takes is taken back out and stays pending); slash commands and unsupported harnesses still wait
    #[arg(long)]
    deliver_while_working: bool,
}
impl Delivery {
    fn options(&self) -> DrainOptions {
        DrainOptions {
            ready_timeout: Duration::from_secs_f64(self.ready_timeout),
            working_timeout: Duration::from_secs_f64(self.working_timeout),
            max_attempts: self.max_attempts,
            deliver_while_working: self.deliver_while_working,
            ..DrainOptions::default()
        }
    }
}

#[derive(Args)]
struct AttachSelf {
    #[command(flatten)]
    agent: Named,
    /// Directory recorded for the session; with no workspace configured, its base name labels the Herdr workspace
    #[arg(long, default_value = ".", value_name = "DIR")]
    cwd: PathBuf,
    /// Existing Herdr workspace ID; default: configured project workspace, else HERDR_WORKSPACE_ID, else the workspace labelled with the --cwd base name; an explicit ID must match configured policy
    #[arg(long, value_name = "ID")]
    workspace_id: Option<String>,
    /// Seconds to wait for the viewer to become the tab's foreground process, greater than zero and at most 300
    #[arg(long, default_value = "30", value_parser = startup_seconds)]
    startup_timeout: f64,
    /// Also designate the registered session as this registry's coordinator
    #[arg(long)]
    as_coordinator: bool,
}

#[derive(Args)]
struct Start {
    #[command(flatten)]
    agent: Named,
    /// Working directory for the new harness
    #[arg(long, default_value = ".", value_name = "DIR")]
    cwd: PathBuf,
    /// Owner-configured profile from CWD/.agentctl/profiles.json, or, when CWD has none, from the
    /// profiles.json beside a registry named .agentctl (so a worktree outside the project uses the
    /// project's profiles). Can combine with --resume for local interactive Claude, Codex, or
    /// Muse; other explicit launch settings conflict
    #[arg(long)]
    profile: Option<String>,
    /// Execution mode; headless workers require an installation with the worker extension
    #[arg(long, value_parser = ["interactive", "headless"])]
    mode: Option<String>,
    /// Terminal host for the new record: herdr or tmux; when given it also selects that terminal backend for this start (default: the backend selected by --term-backend, environment, or profile config)
    #[arg(long, value_parser = ["herdr", "tmux"])]
    backend: Option<String>,
    /// Herdr harness kind; Codex, Claude, and Muse have model presets; agentcloud creates an agentcloud session viewed through agentterm
    #[arg(long)]
    harness: Option<String>,
    /// agentcloud only: session driver passed to agentcloudctl create --harness (default: server default)
    #[arg(long, value_name = "KIND", value_parser = ["native", "claude-code", "codex", "muse-code"])]
    cloud_harness: Option<String>,
    /// agentcloud only: provision a fresh node for the session; the orchestrator holds its lease while the session is live and releases it once idle
    #[arg(long)]
    provision: bool,
    /// agentcloud only: envspec the provisioned node is reserved from, which decides its checkout; requires --provision
    #[arg(long, value_name = "NAME", requires = "provision")]
    envspec: Option<String>,
    /// agentcloud only: roster label for the provisioned node; requires --provision
    #[arg(long, value_name = "TEXT", requires = "provision")]
    purpose: Option<String>,
    /// agentcloud only: absolute session working directory on the node (default: server default); may contain {cwd}, {name}, {host}, or {fqdn}
    #[arg(long, value_name = "PATH")]
    cloud_workspace: Option<String>,
    /// agentcloud only: bind this existing node at creation; conflicts with --provision; may contain {host} or {fqdn} (example: --node-id '{fqdn}')
    #[arg(long, value_name = "ID", conflicts_with = "provision")]
    node_id: Option<String>,
    /// agentcloud only: session title (default: NAME); {name} is the agent name, {host} the local host's first label, {fqdn} its full name, {cwd} the agent's directory (example: --cloud-title '{name}-{host}')
    #[arg(long, value_name = "TEMPLATE")]
    cloud_title: Option<String>,
    /// Model identifier passed unchanged to the Codex or Claude harness
    #[arg(long)]
    model: Option<String>,
    /// Structured harness reasoning effort
    #[arg(long)]
    reasoning_effort: Option<String>,
    /// Existing native conversation ID for local interactive Claude, Codex, or Muse; can combine
    /// with --profile (example: --profile reviewer --resume CONVERSATION_ID)
    #[arg(long, value_name = "SESSION")]
    resume: Option<String>,
    /// Extra literal harness argument; repeat or use --harness-arg=--flag
    #[arg(long = "harness-arg", value_name = "ARG")]
    harness_args: Vec<String>,
    /// Set a literal KEY=VALUE variable in the created interactive Herdr tab; repeatable and omitted from status
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = environment_value)]
    environment: Vec<String>,
    /// Initial prompt to deliver after startup; conflicts with --file
    #[arg(long, conflicts_with = "file", value_name = "TEXT")]
    brief: Option<String>,
    /// UTF-8 file containing the initial prompt; conflicts with --brief
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// Existing Herdr workspace ID; default: configured project workspace, else HERDR_WORKSPACE_ID, else the workspace of the caller's own agentcloud session registered by attach-self, else shared subagents; an explicit ID must match configured policy
    #[arg(long, value_name = "ID")]
    workspace_id: Option<String>,
    /// Interactive only: box the agent to this wrkslots slot (plain-worktree or image-backed); the pane shell is replaced by the line `wrkslots shell-command SLOT` prints (per-slot slice limits and, except with cgroup isolation, a confined file-system view) before the harness starts, and the agent's cwd is the slot directory. Needs wrkslots on PATH or AGENTCTL_WRKSLOTS_BIN
    #[arg(long, value_name = "SLOT")]
    slot: Option<String>,
    /// With --slot: userns = limits plus the file-system view in a user namespace; root = the same view built through sudo -n, for harness launchers that need a setuid step (claude and codex only: the boxed harness runs behind sudo's terminal relay, so agentctl launches it itself, reads its state from Herdr's screen rules, and types prompts with screen verification; goals are unavailable); cgroup = limits only (default: the project's configuration.sandbox.isolation, else userns)
    #[arg(long, value_name = "MODE", value_parser = ["userns", "cgroup", "root"], requires = "slot")]
    slot_isolation: Option<String>,
    /// With --slot: wrkslots project root (default: --cwd, searched upward for .wrkslots.yml)
    #[arg(long, value_name = "DIR", requires = "slot")]
    slot_project: Option<PathBuf>,
    /// Interactive only: run the agent as a (sub)coordinator in a wrkslots coordinator box (`wrkslots shell-command --box`): the same limits and file-system view as --slot, but with no slot, starting in --cwd, with every slot, the wrkslots registry, the slots' Git directories, and the agentctl registry in --cwd writable (or the whole project root with --box-writable project), so it can create slots and launch its own subagents into them. Subagents it launches start outside its box and box themselves into their own slot. Conflicts with --slot. Needs wrkslots on PATH or AGENTCTL_WRKSLOTS_BIN
    #[arg(long, conflicts_with = "slot")]
    project_box: bool,
    /// With --project-box: worktrees = the managed worktrees directory and the slots' Git directories; project = the whole project root (default: the project's configuration.sandbox.coordinator_writable, else worktrees)
    #[arg(long, value_name = "SCOPE", value_parser = ["worktrees", "project"], requires = "project_box")]
    box_writable: Option<String>,
    /// With --project-box: userns, root, or cgroup, as for --slot-isolation; root supports claude and codex only (default: the project's configuration.sandbox.isolation, else userns)
    #[arg(long, value_name = "MODE", value_parser = ["userns", "cgroup", "root"], requires = "project_box")]
    box_isolation: Option<String>,
    /// With --project-box: wrkslots project root (default: --cwd, searched upward for .wrkslots.yml)
    #[arg(long, value_name = "DIR", requires = "project_box")]
    box_project: Option<PathBuf>,
    /// With --project-box: box name, which names its slice and its persistent private $HOME layer (default: the agent name)
    #[arg(long, value_name = "NAME", requires = "project_box")]
    box_name: Option<String>,
    /// Record no created_by or managed_by edge naming the calling agent (default: record them when the caller is a registered agent of this registry; a profile's "record_edges": false does the same)
    #[arg(long)]
    no_edges: bool,
    /// Seconds to wait for harness startup, greater than zero and at most 300
    #[arg(long, default_value = "30", value_parser = startup_seconds)]
    startup_timeout: f64,
    #[command(flatten)]
    delivery: Delivery,
}

#[derive(Args)]
struct Profiles {
    /// Project directory containing .agentctl/profiles.json; when it has none, the profiles.json
    /// beside a registry named .agentctl is listed
    #[arg(long, default_value = ".", value_name = "DIR")]
    cwd: PathBuf,
}

#[derive(Args)]
struct Skill {
    #[command(subcommand)]
    command: SkillCommand,
}

#[derive(Subcommand)]
enum SkillCommand {
    /// Install for Codex, Claude, and Muse
    Install(SkillInstall),
}

#[derive(Args)]
struct SkillInstall {
    /// Target harness; repeat; omission installs all supported harnesses
    #[arg(long, value_parser = ["codex", "claude", "muse"])]
    harness: Vec<String>,
    /// Replace divergent regular content; symlinks are always refused
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct Adopt {
    #[command(flatten)]
    agent: Named,
    /// Exact live pane containing the agent, in the selected backend's spelling (required)
    #[arg(long, value_name = "ID")]
    pane: String,
    /// Expected live workspace (tmux session) label; a mismatch is refused (required)
    #[arg(long, value_name = "LABEL")]
    workspace: String,
    /// Expected live agent working directory; compared canonically (required)
    #[arg(long, value_name = "DIR")]
    cwd: PathBuf,
    /// Expected live harness kind, such as codex, claude, or muse (required)
    #[arg(long, value_name = "KIND")]
    harness: String,
    /// Stable native conversation ID already reported by this exact pane
    #[arg(long, value_name = "ID")]
    session: Option<String>,
    /// Record no managed_by edge naming the calling agent (default: the adopting agent manages the adopted one, when it is a registered agent of this registry)
    #[arg(long)]
    no_edges: bool,
    /// Also designate the adopted agent as this registry's coordinator
    #[arg(long)]
    as_coordinator: bool,
}

#[derive(Args)]
struct Prompt {
    /// Prompt text; use --file for a multiline or large prompt
    #[arg(conflicts_with = "file")]
    text: Option<String>,
    /// UTF-8 file containing prompt text
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
}
impl Prompt {
    fn read(&self) -> Result<Option<String>, String> {
        let text = self.file.as_ref().map_or_else(
            || Ok(self.text.clone()),
            |path| {
                fs::read_to_string(path)
                    .map(Some)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))
            },
        )?;
        if text.as_deref().is_some_and(|text| text.trim().is_empty()) {
            return Err("instruction must not be empty".to_owned());
        }
        Ok(text)
    }

    fn read_bounded(&self, maximum: usize, label: &str) -> Result<Option<String>, String> {
        let text = match self.file.as_ref() {
            Some(path) => {
                let file = fs::File::open(path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
                let limit = u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1);
                let mut bytes = Vec::with_capacity(maximum.saturating_add(1));
                file.take(limit)
                    .read_to_end(&mut bytes)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
                if bytes.len() > maximum {
                    return Err(format!("{label} exceeds {maximum} UTF-8 bytes"));
                }
                Some(
                    String::from_utf8(bytes)
                        .map_err(|_| format!("{} does not contain valid UTF-8", path.display()))?,
                )
            }
            None => self.text.clone(),
        };
        if text.as_deref().is_some_and(|text| text.trim().is_empty()) {
            return Err(format!("{label} must not be empty"));
        }
        if text.as_ref().is_some_and(|text| text.len() > maximum) {
            return Err(format!("{label} exceeds {maximum} UTF-8 bytes"));
        }
        Ok(text)
    }
}

#[derive(Args)]
struct Send {
    #[command(flatten)]
    agent: Named,
    #[command(flatten)]
    prompt: Prompt,
    /// Stable caller-supplied ID; an ID already present in any delivery state is refused
    #[arg(long, value_name = "ID")]
    message_id: Option<String>,
    /// Model override for a headless turn (requires an installation with the worker extension)
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,
    /// Seconds after which no drain types the prompt; an expired prompt is set aside in the queue's cancelled/ untyped. The prompt goes to the queue's inbox.v2/, which older agentctl binaries and the other edition never type (default: no expiry; interactive agents only)
    #[arg(long, value_name = "SECONDS", value_parser = positive_seconds)]
    ttl: Option<f64>,
    #[command(flatten)]
    delivery: Delivery,
}
#[derive(Args)]
struct Drain {
    #[command(flatten)]
    agent: Named,
    /// Seconds after the drain starts after which it starts no further prompt; a prompt being typed is not interrupted, and readiness waits end by then (default: no limit)
    #[arg(long, value_name = "SECONDS", value_parser = positive_seconds)]
    deadline: Option<f64>,
    /// Seconds to wait for the queue's delivery lock and the host-wide target lock before giving up with exit 75 and nothing typed (default: wait until free)
    #[arg(long, value_name = "SECONDS", value_parser = positive_seconds)]
    lock_timeout: Option<f64>,
    /// Most prompts to type in this drain; the rest stay pending (default: no limit)
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    max_items: Option<u64>,
    #[command(flatten)]
    delivery: Delivery,
}
#[derive(Args)]
struct Cancel {
    #[command(flatten)]
    agent: Named,
    /// Queue message ID to cancel
    #[arg(value_name = "MESSAGE_ID")]
    message_id: String,
    /// Why, recorded in the tombstone and in the cancelled item (default: cancelled)
    #[arg(long, default_value = "cancelled", value_name = "TEXT")]
    reason: String,
}
#[derive(Args)]
struct Read {
    #[command(flatten)]
    agent: Named,
    /// Maximum terminal lines to read
    #[arg(long, default_value = "500", value_parser = clap::value_parser!(u32).range(1..=1_000_000))]
    lines: u32,
    /// Output boundary; last reads an agentcloud agent's final reply or a headless transcript; since_turn requires headless transcripts
    #[arg(long, default_value = "tail", value_parser = ["tail", "all", "last", "since_turn"])]
    output: String,
    /// First included headless turn; requires the worker extension and --output since_turn
    #[arg(long, value_name = "NUMBER")]
    since_turn: Option<u64>,
}
#[derive(Args)]
struct Wait {
    #[command(flatten)]
    agent: Named,
    /// Seconds to wait for readiness (0 probes immediately)
    #[arg(long, default_value = "900", value_parser = seconds)]
    timeout: f64,
}
#[derive(Args)]
struct Goal {
    #[command(flatten)]
    agent: Named,
    #[command(flatten)]
    prompt: Prompt,
    /// JSON argv array for a native Codex goal-reader command
    #[arg(long, value_name = "JSON", value_parser = command_json)]
    goal_command_json: Option<GoalCommand>,
    #[command(flatten)]
    delivery: Delivery,
}
#[derive(Args)]
struct BindSession {
    #[command(flatten)]
    agent: Named,
    /// Native conversation ID verified by the operator
    session: String,
    /// JSON argv array for a native Codex goal-reader command
    #[arg(long, value_name = "JSON", value_parser = command_json)]
    goal_command_json: Option<GoalCommand>,
}

#[derive(Args)]
struct Chat {
    #[command(subcommand)]
    command: ChatCommand,
}

#[derive(Subcommand)]
enum ChatCommand {
    /// Print the shortest safe setup for the event-driven chat bridge
    Quickstart,
    /// Print the chat bridge configuration and operations guide
    Userguide,
    /// Validate plugin, target, helper, and config authority, then create state
    Init(ChatInit),
    /// Inspect durable bridge state without contacting Herdr or a provider
    #[command(
        after_help = "Example:\n  agentctl --registry ~/project/.agentctl chat status --bridge-state ~/.local/state/agentctl/chat\n\nBesides the bridge's own records, delivery_alarm.agent_lock says who holds the bridged agent's\nlifecycle lock in --registry (from /proc/locks) and which senders are waiting for it, longest first,\nand each stalled request that has not reached the agent's queue gets waiting_on, one sentence\nnaming the holder and the waiters. Every managed send holds that lock until its delivery is\nconfirmed, so a request can wait behind other agents' messages; the bridge's own sends wait as\npriority senders and go ahead of ordinary agentctl sends. Pass the registry the bridge runs with."
    )]
    Status(ChatState),
    /// Grant explicit senders temporary permission to message the bridge
    #[command(
        after_help = "Examples:\n  agentctl chat grant --bridge-state DIR U0GUEST001 --for 12h --note 'project discussion'\n  agentctl chat grant --bridge-state DIR member@example.org --for 7d\n  agentctl chat grant --bridge-state DIR --all-members --for 30m\n\nEvery grant needs a positive duration. The default configurable maximum is 30 days.\nNames need an outbound helper supporting resolve_sender; raw provider IDs work without it.\n--all-members enumerates every configured channel now, lists each saved ID, and never grants\na wildcard or future members. A running bridge refreshes its sender list after the current\nprovider item or heartbeat; expiry and revocation are checked again for each batch."
    )]
    Grant(ChatGrant),
    /// List permanent owners and active or expired grants, with time remaining
    #[command(
        after_help = "Example: agentctl chat grants --bridge-state DIR\n\nRead-only; never contacts Herdr or the provider. Expired grants stay listed until revoked or\nrenewed. Times in --json are Unix milliseconds and remaining_seconds is zero when expired."
    )]
    Grants(ChatGrants),
    /// Revoke temporary sender grants; permanent owners cannot be removed
    #[command(
        after_help = "Example: agentctl chat revoke --bridge-state DIR U0GUEST001\n\nUse provider IDs or names resolved by the configured helper. Revocation applies to new\nmessages immediately at admission, including on an already open provider subscription."
    )]
    Revoke(ChatRevoke),
    /// Change max_grant_seconds in the existing bridge config without re-initializing it
    #[command(
        after_help = "Example: agentctl chat grant-limit --bridge-state DIR --for 30d\n\nThe default maximum is 30 days. This sets the limit for future grants and renewals; already\nissued grants keep their recorded expiry. The service does not need to restart."
    )]
    GrantLimit(ChatGrantLimit),
    /// Approve one unchanged checkpoint-only gap retry while the service is stopped
    RetryCheckpointGap(ChatRetryGap),
    /// Approve an exact committed-boundary replay, including messages, with the runner stopped
    #[command(
        after_help = "Example: agentctl chat retry-boundary-gap --bridge-state /home/me/chat-state --expected-gap-sha256 <SHA256> --expected-checkpoint-sha256 <SHA256> --expected-configuration-sha256 <SHA256> --keep-cursor <CURSOR> --evidence-file /home/me/private/evidence.json --evidence-sha256 <SHA256>\n\nThis preserves the cursor and unresolved gap. Recovery requires a newer exact provider commit; changed message payloads or a replacement checkpoint are refused."
    )]
    RetryBoundaryGap(ChatRetryGap),
    /// Accept a provider checkpoint in place of a saved boundary whose messages are all stored
    #[command(
        after_help = "Example: agentctl chat accept-checkpoint-boundary --bridge-state /home/me/chat-state --keep-cursor <CURSOR> --expected-checkpoint-sha256 <SHA256> --expected-configuration-sha256 <SHA256>\n\nUse it when `chat status` reports a boundary_mismatch whose offered batch is a checkpoint only: the provider can resume at the saved inclusive cursor only as a checkpoint, while the saved boundary there also carried messages. The command refuses unless the service is stopped, the pins match, the cursor is the current one, no gap is unresolved, the boundary's provider commit is confirmed, and every saved boundary message is a stored request that is acknowledged and was delivered or replied to (or a retired request). It writes one audit under boundary-acceptances/ and removes boundary-mismatch.json; start the service afterwards. The cursor never moves backwards or skips: the provider's checkpoint at this cursor is admitted once and delivery resumes after it."
    )]
    AcceptCheckpointBoundary(ChatAcceptCheckpointBoundary),
    /// Inspect one exact active retained request without writes or external access
    Inspect(ChatInspect),
    /// Print one thread's retained messages as quoted text, oldest first, without writes
    #[command(
        after_help = "Example:\n  agentctl chat thread --bridge-state ~/.local/state/agentctl/chat \\\n    --thread spaces/example/threads/one --last 20\n\nA request prompt for a reply in an existing thread prints this command with the service's own\nexecutable, its exact state directory, and the thread. The output lists the thread's retained requests and the replies captured\nfor them, each with its UTC time and age. It drops the line breaks at the end of each request or\nreply and prefixes every line of message text with `> `, or an empty line with `>` alone.\nThe bridge retains only requests it admitted from allowed senders that have not been retired, so\nthe provider's own thread remains the complete record. It reads under the shared state lock\nand never writes state or contacts Herdr, a helper, or a provider."
    )]
    Thread(ChatThread),
    /// List the replies the bridge holds, newest first: what was sent, when, and as which message
    #[command(
        after_help = "Examples:\n  agentctl chat sent --bridge-state ~/.local/state/agentctl/chat\n  agentctl chat sent --bridge-state ~/.local/state/agentctl/chat --request <KEY> --json\n\nEach reply shows when it was sent (or captured, while unsent, or for a record kept before send\ntimes were recorded, which it says) in UTC and how long ago, its\nnumber within its request, the request key, the chat message and thread it answers, and its\noutcome: `sent as <provider message ID>` once the chat provider accepted it, `captured, not yet\nsent`, or `send in progress or outcome unknown`, which the service retries with the same\noperation ID so it is never posted twice. The first 300 characters of each reply follow, every\nline prefixed with `> `. --json prints one document (schema agentctl-chat-sent/v1) with the\nsame fields, times as Unix milliseconds, and each reply's whole text.\n\nThe bridge holds only the replies of requests that are not retired, so the chat thread itself\nis the complete record. It reads under the shared state lock and never writes state or\ncontacts Herdr, a helper, or a provider."
    )]
    Sent(ChatSent),
    /// Publish one explicit operator message through the configured helper: a new root message,
    /// or with --thread-id a reply in an existing thread
    #[command(
        after_help = "Examples:\n  agentctl chat publish --bridge-state ~/.local/state/agentctl/chat \\\n    --channel-id spaces/example --request-id \"$(uuidgen | tr A-F a-f)\" --file status.md\n  agentctl chat publish --bridge-state ~/.local/state/agentctl/chat \\\n    --channel-id spaces/example --thread-id spaces/example/threads/one \\\n    --request-id \"$(uuidgen | tr A-F a-f)\" --file check.md\n\nWithout --thread-id the message starts a new thread. With it, the message is a reply in that\nthread, which must be named under the channel (<channel>/threads/<id>); it may be a thread\nthe operator started with an earlier publish. Use the provider's thread name for the root\nmessage: in Slack the thread of <channel>/messages/<ts> is <channel>/threads/<ts>.\n\nOn success it prints one JSON object: version, id (the request UUID), action (send), ok, and\nreceipt.message_id, the provider's name for the posted message; a threaded publish adds\nthread_id. Retry an uncertain outcome only with the identical UUID, channel, thread and text.\nIt neither writes bridge state nor creates a reply route."
    )]
    Publish(ChatPublish),
    /// Run one bounded delivery, terminal-capture, and outbound recovery pass; its sends wait for
    /// the agent's lifecycle lock as priority senders, ahead of ordinary agentctl sends
    Tick(ChatOperate),
    /// Run event-driven provider and Herdr subscriptions until SIGINT or SIGTERM; the bridge's
    /// sends wait for the agent's lifecycle lock as priority senders, ahead of ordinary agentctl
    /// sends, so a person's message waits at most for the delivery in progress
    Run(ChatRun),
    /// Store one reply for an open request from a file, and wake a running service to send it.
    /// Normally an agent replies between the prompt's two marker lines instead
    #[command(
        after_help = "Example:\n  agentctl chat reply --bridge-state ~/.local/state/agentctl/chat \\\n    --request <KEY> --reply-id 001 --file reply.md\n\nUse this command only when the two marker lines cannot do the job. A reply printed between\nthe request's two marker lines is the normal way: it reaches the user in the chat and also stays\nin the agent's terminal, where the owner may be reading. Use the command to send a reply before\nthe agent's turn ends, such as a progress update during long work, or to send again a reply that\nwas written between the lines but did not reach the chat: one reported as not sent, or one that\n`agentctl chat sent` does not list, as when the screen was redrawn before the service read it.\nSend each reply one way only. A copy printed in the terminal without the marker lines sends\nnothing, so it is a safe way to show a reply sent by the command.\n\nWhen `chat run` has --offer-reply-command and outbound replies are enabled, each request prompt\nprints this command with the service's own executable, the absolute path of its state\ndirectory, the request key, and the reply ID, unless no file is left at that executable's path\nor one of those words cannot be printed safely; then the prompt gives only the two marker\nlines.\n\nThe command stores the file's text as a reply of that request, as if the service had read it\nbetween the reply ID's two marker lines on the agent's screen, and the service sends it like\nany reply it reads. Line breaks at the end of the file are dropped. A text the request already\nholds is not stored again, so running the command twice sends it once. It then asks a service\nrunning on the same state directory to send the reply at once; a service that cannot listen\nsends it at its next reconciliation, when the agent goes idle, or when it starts.\n\nOn success it prints one JSON object: request, reply_id, outcome (stored or already_stored),\nordinal, phase, and service_woken, which says whether the wake was sent to a socket of the\ncurrent user in the state directory; nothing confirms that a service received it. Exit\nstatus: 0 when the request holds the reply; 1 when the reply is refused, with the reason on\nstandard error and nothing stored, as for an empty or blank text or one over 30,000 bytes;\n1 also when the state cannot be read or written, or when the result cannot be printed after\nthe reply was stored; 75 when nothing was stored but the same command can succeed later; 2\nfor a usage error, which includes a file that cannot be read or is not UTF-8. It never\ncontacts Herdr, a helper, or a provider."
    )]
    Reply(ChatReply),
    /// Stop accepting fenced replies for one exact retained request
    Close(ChatClose),
    /// Read every retained reply body for one request (explicit deep audit; cost is linear in its history)
    #[command(
        after_help = "Example: agentctl chat history --bridge-state ~/.local/state/agentctl/chat --request <KEY-OR-PREFIX>"
    )]
    History(ChatHistory),
    /// Read one page of prior thread messages for a saved request through the configured adapter
    #[command(
        after_help = "Example: agentctl chat context --bridge-state ~/.local/state/agentctl/chat --request <KEY-OR-PREFIX> --limit 10"
    )]
    Context(ChatContext),
    /// Internal systemd ExecStop helper for one inode-bound main-process pidfd
    #[command(hide = true)]
    GracefulStopMain(ChatGracefulStopMain),
}

#[derive(Args)]
struct ChatState {
    /// Private durable bridge state directory
    #[arg(long, value_name = "DIR")]
    bridge_state: PathBuf,
}

#[derive(Args)]
struct ChatGrant {
    #[command(flatten)]
    state: ChatState,
    /// Provider sender IDs, or names/emails resolved by the configured helper
    #[arg(value_name = "SENDER", required_unless_present = "all_members", conflicts_with = "all_members", num_args = 1..)]
    senders: Vec<String>,
    /// Enumerate and grant the current members of all configured channels explicitly
    #[arg(long)]
    all_members: bool,
    /// Required lifetime: positive whole number with s, m, h, d or w (30m, 12h, 7d)
    #[arg(long = "for", value_name = "DURATION", value_parser = crate::chat_runtime::parse_grant_duration)]
    duration: Duration,
    /// Optional reason retained with each grant (at most 2000 UTF-8 bytes)
    #[arg(long, value_name = "TEXT")]
    note: Option<String>,
}

#[derive(Args)]
struct ChatGrants {
    #[command(flatten)]
    state: ChatState,
    /// Print one JSON document instead of the text listing
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ChatRevoke {
    #[command(flatten)]
    state: ChatState,
    /// Provider sender IDs, or names/emails resolved by the configured helper
    #[arg(value_name = "SENDER", required = true, num_args = 1..)]
    senders: Vec<String>,
}

#[derive(Args)]
struct ChatGrantLimit {
    #[command(flatten)]
    state: ChatState,
    /// Maximum lifetime for future grants: positive whole number with s, m, h, d or w
    #[arg(long = "for", value_name = "DURATION", value_parser = crate::chat_runtime::parse_grant_duration)]
    duration: Duration,
}

#[derive(Args)]
struct ChatSent {
    #[command(flatten)]
    state: ChatState,
    /// Only this request's replies: its exact 64-character lowercase hexadecimal key
    #[arg(long, value_name = "KEY")]
    request: Option<String>,
    /// Print this many of the most recent replies, newest first (1-100)
    #[arg(
        long,
        value_name = "N",
        default_value_t = crate::chat_runtime::DEFAULT_THREAD_HISTORY_MESSAGES,
        value_parser = clap::value_parser!(u32)
            .range(1..=i64::from(crate::chat_runtime::MAX_THREAD_HISTORY_MESSAGES))
    )]
    last: u32,
    /// Print one JSON document with each reply's whole text instead of the text listing
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ChatInit {
    #[command(flatten)]
    state: ChatState,
    /// Private owner-only JSON bridge configuration (maximum 1 MiB)
    #[arg(long, value_name = "FILE")]
    config: PathBuf,
}

#[derive(Args)]
struct ChatInspect {
    #[command(flatten)]
    state: ChatState,
    /// Exact 64-character lowercase hexadecimal request key
    #[arg(long)]
    request: String,
}

#[derive(Args)]
struct ChatThread {
    #[command(flatten)]
    state: ChatState,
    /// Exact provider thread ID; the command a request prompt prints already quotes it for the shell
    #[arg(long, value_name = "THREAD", allow_hyphen_values = true)]
    thread: String,
    /// Print this many of the thread's most recent retained messages, oldest first (1-100)
    #[arg(
        long,
        value_name = "N",
        default_value_t = crate::chat_runtime::DEFAULT_THREAD_HISTORY_MESSAGES,
        value_parser = clap::value_parser!(u32)
            .range(1..=i64::from(crate::chat_runtime::MAX_THREAD_HISTORY_MESSAGES))
    )]
    last: u32,
}

#[derive(Args)]
struct ChatRetryGap {
    #[command(flatten)]
    state: ChatState,
    /// SHA256 of the reviewed unresolved gap.json bytes
    #[arg(long, value_name = "SHA256")]
    expected_gap_sha256: String,
    /// SHA256 of the reviewed checkpoint.json bytes
    #[arg(long, value_name = "SHA256")]
    expected_checkpoint_sha256: String,
    /// SHA256 of the reviewed bridge.json configuration bytes
    #[arg(long, value_name = "SHA256")]
    expected_configuration_sha256: String,
    /// Exact existing opaque cursor to preserve; this command cannot advance it
    #[arg(long, value_name = "CURSOR")]
    keep_cursor: String,
    /// Private JSON object of operator-reviewed provider evidence (maximum 64 KiB)
    #[arg(long, value_name = "FILE")]
    evidence_file: PathBuf,
    /// SHA256 of the exact reviewed evidence file bytes
    #[arg(long, value_name = "SHA256")]
    evidence_sha256: String,
}

#[derive(Args)]
struct ChatAcceptCheckpointBoundary {
    #[command(flatten)]
    state: ChatState,
    /// SHA256 of the reviewed checkpoint.json bytes
    #[arg(long, value_name = "SHA256")]
    expected_checkpoint_sha256: String,
    /// SHA256 of the reviewed bridge.json configuration bytes
    #[arg(long, value_name = "SHA256")]
    expected_configuration_sha256: String,
    /// Exact current opaque cursor; this command cannot select another
    #[arg(long, value_name = "CURSOR")]
    keep_cursor: String,
}

#[derive(Args)]
struct ChatPublish {
    #[command(flatten)]
    state: ChatState,
    /// Exact configured provider channel or space authority
    #[arg(long, value_name = "CHANNEL")]
    channel_id: String,
    /// Existing thread of the channel to reply in, as <channel>/threads/<id> (default: start a
    /// new thread with a root message)
    #[arg(long, value_name = "THREAD", allow_hyphen_values = true)]
    thread_id: Option<String>,
    /// Caller-owned lowercase RFC 4122 version-4 idempotency UUID
    #[arg(long, value_name = "UUID", value_parser = operation_uuid)]
    request_id: String,
    /// Message text; use --file for multiline input
    #[arg(required_unless_present = "file", conflicts_with = "file")]
    text: Option<String>,
    /// UTF-8 file containing the message, read with a 30,000-byte bound
    #[arg(long, value_name = "PATH", required_unless_present = "text")]
    file: Option<PathBuf>,
    /// Slack outbound format for this message: markdown (vendor conversion) or converted
    /// (the helper converts Markdown to Slack mrkdwn itself). Overrides the bridge config
    /// and AGENTCTL_SLACK_FORMAT for this publish only (Slack helper only)
    #[arg(long, value_name = "FORMAT", value_parser = slack_format_value)]
    slack_format: Option<crate::chat_runtime::SlackFormat>,
}

impl ChatPublish {
    fn read_message(&self) -> Result<String, String> {
        Prompt {
            text: self.text.clone(),
            file: self.file.clone(),
        }
        .read_bounded(MAX_CHAT_PUBLISH_BYTES, "chat publish message")?
        .ok_or_else(|| "chat publish requires message text or --file".to_owned())
    }
}

#[derive(Args)]
struct ChatReply {
    #[command(flatten)]
    state: ChatState,
    /// Exact 64-character lowercase hexadecimal request key
    #[arg(long)]
    request: String,
    /// Reply ID that the request's prompt gives in its two marker lines
    #[arg(long, value_name = "ID", allow_hyphen_values = true)]
    reply_id: String,
    /// UTF-8 file containing the reply, read with a 30,000-byte bound
    #[arg(long, value_name = "PATH")]
    file: PathBuf,
}

impl ChatReply {
    /// The reply file's text. A file that cannot be read or is not UTF-8 is a usage error. A text
    /// over the bound is refused like any reply the service refuses, and so is an empty or blank
    /// one, which the store refuses.
    fn read_reply(&self) -> Result<String, Failure> {
        let maximum = crate::chat_runtime::MAX_REPLY_BYTES;
        let unreadable = |error: io::Error| {
            Failure::Usage(format!("cannot read {}: {error}", self.file.display()))
        };
        let file = fs::File::open(&self.file).map_err(unreadable)?;
        let limit = u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1);
        let mut bytes = Vec::with_capacity(maximum.saturating_add(1));
        file.take(limit)
            .read_to_end(&mut bytes)
            .map_err(unreadable)?;
        if bytes.len() > maximum {
            return Err(Failure::Chat(
                crate::chat_service::ChatServiceError::Runtime(
                    crate::chat_runtime::ChatRuntimeError::Invalid(format!(
                        "chat reply exceeds {maximum} UTF-8 bytes"
                    )),
                ),
            ));
        }
        String::from_utf8(bytes).map_err(|_| {
            Failure::Usage(format!(
                "{} does not contain valid UTF-8",
                self.file.display()
            ))
        })
    }
}

#[derive(Args)]
struct ChatRun {
    #[command(flatten)]
    operate: ChatOperate,
    /// Ignore new messages starting with PREFIX after leading whitespace (case-sensitive; repeat up to 32 times; default: none). Prefixes must be nonempty, at most 256 UTF-8 bytes, without control characters; applies only to this run.
    #[arg(long = "ignore-text-prefix", value_name = "PREFIX", value_parser = chat_ignored_text_prefix)]
    ignored_text_prefixes: Vec<String>,
    /// Print an `agentctl chat reply` command in each request prompt that gives the two reply marker lines, as a way to send a reply at once (default: off). A prompt leaves the command out when no file is left at this executable's path or a word of the command cannot be printed safely. With or without this option, a service with outbound replies enabled listens for that command's wakes on the state directory's .wake.sock, unless the socket's absolute path exceeds 107 bytes. The agent must be able to run this executable and write the state directory; applies only to this run.
    #[arg(long)]
    offer_reply_command: bool,
    /// Also type a request into the coordinator's pane while Herdr reports it `working`, when the pane runs Claude Code, and let Claude Code queue it behind the current turn (default: off, which waits for `idle` or `done`). The composer must be empty first, and the submission must be verified: the prompt leaves the composer, with Claude Code's queued-message hint or the prompt shown, and an Enter it drops is pressed again with a doubling wait. A prompt it never accepts is taken back out of the composer and stays pending. Codex and Muse ordinary follow-ups are delivered while working by default (Muse: pasted into its empty editor, where Enter steers the running turn, proved by the new user turn; a prompt Muse never takes is taken back out and stays pending); slash commands and unsupported harnesses still wait; applies only to this run.
    #[arg(long)]
    deliver_while_working: bool,
    /// Clear text left unsubmitted in the coordinator's composer once it has blocked a request and stayed unchanged for DURATION, such as 30m (default: off). Units: s, m or h; a bare number is seconds; at least 1m. The text is first saved with mode 0600 under the state directory's composer-backups/, then deleted with editing keys and the composer proved empty, and then posted once to the first configured channel through the outbound helper with a "[chat-bridge WARNING]" notice; delivery then resumes. Any change to the text restarts the wait, nothing is cleared while the agent works, and a failed clear leaves the text and posts nothing. Requires an outbound helper; applies only to this run.
    #[arg(long, value_name = "DURATION", value_parser = stale_composer_duration)]
    stale_composer_clear_after: Option<Duration>,
    /// Remind a Claude Code coordinator, once, of each chat request whose prompt was typed and that it has not answered, once it has been idle for DURATION since that prompt was typed (default: 60s). Units: s, m or h; a bare number is seconds; at least 10s and at most 12h, and 0 turns reminders off. Idle means Herdr reports the pane idle or done, the pane shows no running turn, and `claude agents --json` reports the pane's session idle. Right before a reminder, the session's transcript (`<session>.jsonl` under the session's CLAUDE_CONFIG_DIR or ~/.claude `projects`) is read, and a request whose reply ID heads a complete reply block in the agent's ordinary text there is left out and never reminded, since that reply was printed though no read of the pane showed it. The reminder names the reply IDs it has received no reply for, says that a reply written only in thinking is not delivered, prints for each the `agentctl chat sent --request` command to check what the bridge holds before sending anything again, and mentions `agentctl chat reply`; it is recorded in reply-reminders.json before it is queued, is typed only into an idle pane and never behind another queued prompt, never through --deliver-while-working, and is taken back out of the queue if it is not typed within 10 seconds (or --ready-timeout if shorter). A reminder taken back holds off the next until the coordinator has been seen working and idle again, or for twice DURATION, doubling with each in a row, up to 60 minutes. Requests delivered before reminders first started for the state directory, or more than 24 hours ago, are never reminded, so DURATION plus the time the coordinator works after a prompt must stay under 24 hours. Needs outbound_enabled; applies only to this run.
    #[arg(long, value_name = "DURATION", default_value = "60s", hide_default_value = true, value_parser = reply_reminder_idle)]
    reply_reminder_idle: ReplyReminderIdle,
    /// Claude Code executable that `--reply-reminder-idle` runs as `PATH agents --json` to tell whether the coordinator is idle (default: claude, looked up on PATH). Each run is killed after 10 seconds, or at once when the service stops; a failed run sends no reminder and is logged. After a run that fails, lists no session in the pane, or takes longer than 3 seconds, Claude Code is not asked again for one --reply-reminder-idle DURATION, except right before a reminder.
    #[arg(
        long,
        value_name = "PATH",
        default_value = "claude",
        hide_default_value = true
    )]
    claude_bin: PathBuf,
}

#[derive(Args)]
struct ChatOperate {
    #[command(flatten)]
    state: ChatState,
    /// Seconds to wait for coordinator readiness; zero keeps daemon passes nonblocking
    #[arg(long, default_value = "0", value_parser = chat_delivery_seconds)]
    ready_timeout: f64,
    /// Positive seconds to wait for evidence a delivered prompt started work
    #[arg(long, default_value = "5", value_parser = positive_chat_delivery_seconds)]
    working_timeout: f64,
    /// Maximum safe terminal-injection attempts for one request
    #[arg(long, default_value = "1", value_parser = clap::value_parser!(u64).range(1..=1_000_000))]
    max_attempts: u64,
    /// Seconds between disk-backed recovery snapshots; provider intake remains event-driven
    #[arg(long, default_value = "300", value_parser = positive_seconds)]
    reconcile_interval: f64,
}

impl ChatOperate {
    fn options(&self) -> crate::chat_service::ServiceOptions {
        crate::chat_service::ServiceOptions {
            delivery: DrainOptions {
                ready_timeout: Duration::from_secs_f64(self.ready_timeout),
                working_timeout: Duration::from_secs_f64(self.working_timeout),
                max_attempts: self.max_attempts,
                // `chat run` sets this from its own flag.
                deliver_while_working: false,
                ..DrainOptions::default()
            },
            reconciliation_interval: Duration::from_secs_f64(self.reconcile_interval),
            timing: crate::chat_service::DeliveryTiming::default(),
            idle_poll_interval: crate::chat_service::IDLE_SATURATED_POLL_INTERVAL,
        }
    }
}

#[derive(Args)]
struct ChatClose {
    #[command(flatten)]
    state: ChatState,
    /// Exact 64-character lowercase hexadecimal request key
    #[arg(long)]
    request: String,
}

#[derive(Args)]
struct ChatContext {
    #[command(flatten)]
    state: ChatState,
    /// Saved request's hexadecimal key or unique prefix of at least 12 characters
    #[arg(long, value_name = "KEY_OR_PREFIX")]
    request: String,
    /// Maximum prior messages per page, 1-200 (default: 10)
    #[arg(long, value_name = "COUNT", default_value_t = 10)]
    limit: i64,
    /// Opaque cursor from the preceding context result to read older messages
    #[arg(long, value_name = "TOKEN")]
    cursor: Option<String>,
}

#[derive(Args)]
struct ChatHistory {
    #[command(flatten)]
    state: ChatState,
    /// Saved request's hexadecimal key or unique prefix of at least 12 characters
    #[arg(long, value_name = "KEY_OR_PREFIX")]
    request: String,
}

#[derive(Args)]
struct ChatGracefulStopMain {
    /// Exact systemd MAINPID value
    #[arg(long)]
    main_pid: u32,
    /// Exact systemd MAINPIDFDID inode value
    #[arg(long)]
    main_pidfd_id: u64,
}

fn seconds(value: &str) -> Result<f64, String> {
    let value: f64 = value.parse().map_err(|_| "expected a number of seconds")?;
    if !value.is_finite() || !(0.0..=31_536_000.0).contains(&value) {
        return Err("seconds must be finite and between 0 and 31536000".to_owned());
    }
    Ok(value)
}
fn positive_seconds(value: &str) -> Result<f64, String> {
    let value = seconds(value)?;
    if value == 0.0 {
        return Err("seconds must be greater than zero".to_owned());
    }
    Ok(value)
}
fn watch_minutes(value: &str) -> Result<u64, String> {
    let value: u64 = value.parse().map_err(|_| "expected a number of minutes")?;
    if !(1..=1440).contains(&value) {
        return Err("minutes must be between 1 and 1440".to_owned());
    }
    Ok(value)
}
fn startup_seconds(value: &str) -> Result<f64, String> {
    let value = seconds(value)?;
    if value <= 0.0 || value > 300.0 {
        return Err("startup seconds must be greater than zero and at most 300".to_owned());
    }
    Ok(value)
}
fn chat_delivery_seconds(value: &str) -> Result<f64, String> {
    let value = seconds(value)?;
    if value > 30.0 {
        return Err("chat delivery seconds must be at most 30".to_owned());
    }
    Ok(value)
}
fn positive_chat_delivery_seconds(value: &str) -> Result<f64, String> {
    let value = chat_delivery_seconds(value)?;
    if value == 0.0 {
        return Err("chat delivery seconds must be greater than zero".to_owned());
    }
    Ok(value)
}
fn environment_value(value: &str) -> Result<String, String> {
    environment_entries(&[value.to_owned()])
        .map(|entries| entries.into_iter().next().expect("one validated entry"))
        .map_err(|error| error.to_string())
}
fn stale_composer_duration(value: &str) -> Result<Duration, String> {
    crate::stale_composer::parse_duration(value)
}
/// `chat run --reply-reminder-idle`: the idle time, or `None` when reminders are off.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReplyReminderIdle(Option<Duration>);
fn reply_reminder_idle(value: &str) -> Result<ReplyReminderIdle, String> {
    crate::chat_service::parse_reply_reminder_idle(value).map(ReplyReminderIdle)
}
fn chat_ignored_text_prefix(value: &str) -> Result<String, String> {
    crate::chat_runtime::validate_ignored_text_prefix(value)
        .map(|()| value.to_owned())
        .map_err(|error| error.to_string())
}
fn slack_format_value(value: &str) -> Result<crate::chat_runtime::SlackFormat, String> {
    crate::chat_runtime::SlackFormat::parse(value)
        .ok_or_else(|| "slack format must be markdown or converted".to_owned())
}
fn operation_uuid(value: &str) -> Result<String, String> {
    crate::chat_runtime::validate_operation_uuid(value)
        .map(|()| value.to_owned())
        .map_err(|error| error.to_string())
}
#[derive(Clone, Debug)]
struct GoalCommand(Vec<String>);
fn command_json(value: &str) -> Result<GoalCommand, String> {
    let command: Vec<String> = serde_json::from_str(value)
        .map_err(|error| format!("expected a JSON argv array: {error}"))?;
    if command.is_empty()
        || command
            .iter()
            .any(|value| value.is_empty() || value.contains('\0'))
    {
        return Err("command must contain nonempty arguments without NUL".to_owned());
    }
    Ok(GoalCommand(command))
}

/// Run the canonical CLI with arguments excluding the executable name.
pub fn main<I: IntoIterator<Item = OsString>>(arguments: I) -> i32 {
    main_with_environment(arguments, &|name| std::env::var(name).ok())
}

/// Run the CLI reading the agentcloud context variables through `environment`, so tests can
/// model running inside or outside an agentcloud session without mutating process state.
pub(crate) fn main_with_environment<I: IntoIterator<Item = OsString>>(
    arguments: I,
    environment: &dyn Fn(&str) -> Option<String>,
) -> i32 {
    let arguments: Vec<OsString> = std::iter::once(OsString::from("agentctl"))
        .chain(arguments)
        .collect();
    if let Some(command) = python_only_command(&arguments) {
        eprintln!(
            "agentctl: {command} is implemented only by the Python edition of agentctl; \
             run it with the Python agentctl (for example DAGRUN_ENGINE=python)"
        );
        return 2;
    }
    let args = match Cli::try_parse_from(arguments.iter().cloned()) {
        Ok(args) => args,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            if code != 0 {
                if let Some(context) = partial_stop_context(&arguments) {
                    print_stop_recovery(&context, &RecoveryAction::Doctor);
                }
            }
            return code;
        }
    };
    let stop_context = matches!(&args.command, Some(Commands::Stop(_)))
        .then(|| StopContext::new(&args.registry, &args.herdr_bin));
    let service_log = args.writes_service_log();
    match run(args, environment) {
        Ok(code) => code,
        Err(Failure::Agent(error)) => emit_agent_failure(
            service_log,
            &error,
            stop_context.as_ref(),
            &RecoveryAction::Doctor,
        ),
        Err(Failure::Stop(failure)) => emit_agent_failure(
            service_log,
            &failure.error,
            stop_context.as_ref(),
            &failure.recovery,
        ),
        Err(Failure::Usage(error)) => {
            if let Some(context) = &stop_context {
                print_failure(service_log, format_args!("{}", stop_reason(&error)));
                print_stop_recovery(context, &RecoveryAction::Doctor);
            } else {
                print_failure(service_log, format_args!("{error}"));
            }
            2
        }
        Err(Failure::Output(error)) => {
            print_failure(service_log, format_args!("cannot write output: {error}"));
            if let Some(context) = &stop_context {
                print_stop_recovery(context, &RecoveryAction::Doctor);
            }
            1
        }
        Err(Failure::Chat(error)) => {
            print_failure(service_log, format_args!("{error}"));
            if let Some(context) = &stop_context {
                print_stop_recovery(context, &RecoveryAction::Doctor);
            }
            1
        }
        Err(Failure::Inbox(error)) => {
            print_failure(service_log, format_args!("{error}"));
            if let Some(context) = &stop_context {
                print_stop_recovery(context, &RecoveryAction::Doctor);
            }
            error.exit_code()
        }
    }
}

// Commands only the Python edition implements: the MCP server, the headless-worker operations,
// and the polling Chat launcher and its history/context readers. This edition refuses them with
// exit status 2 and names the Python opt-in, instead of Clap's unrecognised-subcommand error.
const PYTHON_ONLY_COMMANDS: [&str; 2] = ["migrate", "repair"];
const PYTHON_ONLY_CHAT_COMMANDS: [&str; 1] = ["launch"];
// Global options whose value is the following argument.
const GLOBAL_VALUE_OPTIONS: [&str; 7] = [
    "--registry",
    "--state",
    "--herdr-bin",
    "--agentcloudctl-bin",
    "--agentterm-bin",
    "--agentcloud-url",
    "--from-session",
];

// The display name of the Python-only command in `arguments` (program name first), if any.
// Anything the scan does not recognise is left for Clap to judge.
fn python_only_command(arguments: &[OsString]) -> Option<String> {
    fn next_command(arguments: &[OsString], mut index: usize) -> Option<(&str, usize)> {
        while index < arguments.len() {
            let token = arguments[index].to_str()?;
            if token == "--" {
                // Clap reads the next word in the (sub)command position.
                index += 1;
                return arguments
                    .get(index)
                    .and_then(|word| word.to_str())
                    .map(|word| (word, index));
            }
            if let Some(option) = token.strip_prefix("--") {
                if matches!(option, "help" | "version") {
                    // Clap answers these itself and never reaches a subcommand.
                    return None;
                }
                if option.contains('=') || option == "userguide" {
                    index += 1;
                } else if GLOBAL_VALUE_OPTIONS.contains(&token) {
                    index += 2;
                } else {
                    return None;
                }
            } else if token.starts_with('-') && token != "-" {
                return None;
            } else {
                return Some((token, index));
            }
        }
        None
    }

    let (command, index) = next_command(arguments, 1)?;
    if command == "help" {
        // `agentctl help <command>`: the word after `help` names the command.
        let (target, target_index) = next_command(arguments, index + 1)?;
        if PYTHON_ONLY_COMMANDS.contains(&target) {
            return Some(target.to_owned());
        }
        if target == "chat" {
            let (subcommand, _) = next_command(arguments, target_index + 1)?;
            if PYTHON_ONLY_CHAT_COMMANDS.contains(&subcommand) {
                return Some(format!("chat {subcommand}"));
            }
        }
        return None;
    }
    if PYTHON_ONLY_COMMANDS.contains(&command) {
        return Some(command.to_owned());
    }
    if command == "chat" {
        let (subcommand, subcommand_index) = next_command(arguments, index + 1)?;
        if PYTHON_ONLY_CHAT_COMMANDS.contains(&subcommand) {
            return Some(format!("chat {subcommand}"));
        }
        if subcommand == "help" {
            let (target, _) = next_command(arguments, subcommand_index + 1)?;
            if PYTHON_ONLY_CHAT_COMMANDS.contains(&target) {
                return Some(format!("chat {target}"));
            }
        }
    }
    None
}

fn partial_stop_context(arguments: &[OsString]) -> Option<StopContext> {
    let matches = Cli::command()
        .ignore_errors(true)
        .try_get_matches_from(arguments.iter().cloned())
        .ok()?;
    if matches.subcommand_name() != Some("stop") {
        return None;
    }
    Some(StopContext::new(
        matches
            .get_one::<PathBuf>("registry")
            .map_or(Path::new(".agentctl"), PathBuf::as_path),
        matches
            .get_one::<PathBuf>("herdr_bin")
            .map_or(Path::new("herdr"), PathBuf::as_path),
    ))
}

fn print_stop_recovery(context: &StopContext, recovery: &RecoveryAction) {
    eprintln!("Recovery command: {}", context.command(recovery));
}

fn emit_agent_failure(
    service_log: bool,
    error: &AgentError,
    stop_context: Option<&StopContext>,
    recovery: &RecoveryAction,
) -> i32 {
    if let Some(message) = error.undelivered() {
        if let Err(output) = write_json(
            &json!({"outcome": error.outcome().map(QueueOutcome::as_str), "message_id": message.message_id, "artifact": message.artifact, "error": error.to_string(), "safe_to_retry": error.safe_to_retry()}),
        ) {
            print_failure(service_log, format_args!("{output}"));
            if let Some(context) = stop_context {
                print_stop_recovery(context, recovery);
            }
            return 1;
        }
    }
    if let Some(context) = stop_context {
        print_failure(service_log, format_args!("{}", stop_reason(error)));
        print_stop_recovery(context, recovery);
    } else if error.undelivered().is_none() {
        print_failure(service_log, format_args!("{error}"));
    }
    error.exit_code()
}

impl Cli {
    /// Whether standard error is the chat bridge service log: `chat run` and its stop helper.
    fn writes_service_log(&self) -> bool {
        matches!(
            &self.command,
            Some(Commands::Chat(Chat {
                command: ChatCommand::Run(_) | ChatCommand::GracefulStopMain(_),
            }))
        )
    }
}

/// Print the final error line. In the service log it starts with the UTC time, like every other
/// service log line, so the log shows when the service stopped as well as why.
fn print_failure(service_log: bool, message: std::fmt::Arguments<'_>) {
    if service_log {
        crate::chat_service::service_log(format_args!("agentctl: {message}"));
    } else {
        eprintln!("agentctl: {message}");
    }
}

enum Failure {
    Agent(AgentError),
    Stop(StopFailure),
    Usage(String),
    Output(io::Error),
    Chat(crate::chat_service::ChatServiceError),
    Inbox(crate::inbox::InboxError),
}
impl From<AgentError> for Failure {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
    }
}
impl From<StopFailure> for Failure {
    fn from(error: StopFailure) -> Self {
        Self::Stop(error)
    }
}
fn write_json(value: &impl Serialize) -> io::Result<()> {
    let mut output = io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, value)?;
    output.write_all(b"\n")
}

fn write_text(text: &str) -> io::Result<()> {
    let mut output = io::stdout().lock();
    output.write_all(text.as_bytes())?;
    output.flush()
}

fn plugin_discovery_required(command: &Commands) -> bool {
    matches!(command, Commands::Capabilities)
}

fn run(args: Cli, environment: &dyn Fn(&str) -> Option<String>) -> Result<i32, Failure> {
    if args.userguide {
        print!("{}", crate::USER_GUIDE);
        return Ok(0);
    }
    let Some(command) = args.command else {
        Cli::command().print_help().map_err(Failure::Output)?;
        println!();
        return Ok(0);
    };
    // Directory and manifest I/O is lazy. Session-control commands never inspect the user plugin
    // home; capability inspection and an actual plugin runtime startup own discovery.
    let plugin_inventory = plugin_discovery_required(&command).then(crate::plugins::discover);
    if matches!(
        command,
        Commands::Chat(_)
            | Commands::Inbox(_)
            | Commands::Restore(_)
            | Commands::Watch(_)
            | Commands::WatchAgents(_)
            | Commands::InstallUnits(_)
    ) {
        // These remain Herdr services on the tmux backend too; refuse an
        // explicitly selected tmux here rather than silently driving Herdr.
        // A merely detected tmux (no Herdr installed) falls through so the
        // Herdr client reports its usual not-found error.
        let (selected, explicit) = crate::term::select_backend(
            args.term_backend.as_deref(),
            environment,
            &args.registry,
            &args.herdr_bin,
            &args.tmux_bin,
        )
        .map_err(AgentError::from)?;
        if explicit && selected == term_backend::BackendKind::Tmux {
            return Err(Failure::Usage(
                "this command is a Herdr service and does not run on the tmux terminal backend; \
                 select Herdr with --term-backend herdr"
                    .to_owned(),
            ));
        }
    }
    match command {
        Commands::Capabilities => {
            let (selected, _explicit) = crate::term::select_backend(
                args.term_backend.as_deref(),
                environment,
                &args.registry,
                &args.herdr_bin,
                &args.tmux_bin,
            )
            .map_err(AgentError::from)?;
            write_json(&capabilities_document(
                args.registry,
                plugin_inventory.expect("capabilities requires plugin discovery"),
                selected.as_str(),
            ))
            .map_err(Failure::Output)?;
            return Ok(0);
        }
        Commands::Quickstart => {
            print!("{}", crate::QUICKSTART);
            return Ok(0);
        }
        Commands::Userguide => {
            print!("{}", crate::USER_GUIDE);
            return Ok(0);
        }
        Commands::Mcp => {
            return Ok(crate::mcp::serve(
                args.registry.to_string_lossy().as_ref(),
                args.herdr_bin.to_string_lossy().as_ref(),
            ));
        }
        Commands::Chat(value) => {
            return run_chat(args.registry, args.herdr_bin, value);
        }
        Commands::Inbox(value) => {
            return crate::inbox::run(
                &args.registry,
                &args.herdr_bin,
                &args.agentcloudctl_bin,
                value,
            )
            .map_err(Failure::Inbox);
        }
        Commands::Restore(value) => {
            let client = HerdrClient::with_executable("direct", &args.herdr_bin)
                .map_err(AgentError::from)?;
            let mut config = crate::restore::RestoreConfig::load(&args.registry)?;
            let designation_problem = designation_problem(&client, &args.registry, &mut config);
            let mut report = crate::restore::restore(
                &client,
                &args.registry,
                &crate::restore::SystemHost,
                &config,
                &crate::restore::RestoreOptions {
                    force: value.force,
                    dry_run: value.dry_run,
                    startup_timeout: Duration::from_secs_f64(value.startup_timeout),
                    wrkslots: crate::subagents::project_wrkslots(&args.registry),
                },
            )?;
            report["trigger"] = json!(if value.boot { "boot" } else { "manual" });
            if let Some(problem) = designation_problem {
                report["designation_problem"] = json!(problem);
            }
            write_json(&report).map_err(Failure::Output)?;
            let done = report["action"] == "already-restored"
                || (report["complete"] == true && report["refused"] == 0);
            return Ok(if done || value.dry_run { 0 } else { 75 });
        }
        Commands::Watch(value) => {
            let client = HerdrClient::with_executable("direct", &args.herdr_bin)
                .map_err(AgentError::from)?;
            let mut config = crate::restore::RestoreConfig::load(&args.registry)?;
            if let Some(problem) = designation_problem(&client, &args.registry, &mut config) {
                eprintln!("agentctl watch: {problem}");
            }
            let health = crate::restore::watch(
                &client,
                &args.registry,
                &crate::restore::SystemHost,
                &config,
                &crate::restore::WatchOptions {
                    interval: Duration::from_secs(value.interval),
                    once: value.once,
                    check_only: value.check_only,
                    wrkslots: crate::subagents::project_wrkslots(&args.registry),
                    state_dir: value.state_dir,
                },
            )?;
            write_json(&health).map_err(Failure::Output)?;
            return Ok(if health["ok"] == true { 0 } else { 1 });
        }
        Commands::WatchAgents(value) => {
            let client = HerdrClient::with_executable("direct", &args.herdr_bin)
                .map_err(AgentError::from)?;
            let markers = if value.done_markers.is_empty() {
                vec!["DONE:".to_owned(), "BLOCKED:".to_owned()]
            } else {
                value.done_markers
            };
            let coordinator = match value.coordinator.clone() {
                Some(coordinator) => coordinator,
                None => ManagedAgents::new(&client, &args.registry)?
                    .designated_coordinator()?
                    .unwrap_or_else(|| "coordinator".to_owned()),
            };
            let options = crate::agent_watch::WatchAgentsOptions {
                coordinator,
                interval: Duration::from_secs(value.interval),
                once: value.once,
                check_only: value.check_only,
                approval: Duration::from_secs(value.approval_minutes * 60),
                idle: Duration::from_secs(value.idle_minutes * 60),
                child_done: Duration::from_secs(value.child_done_seconds),
                stall: Duration::from_secs(value.stall_minutes * 60),
                dead: Duration::from_secs(value.dead_minutes * 60),
                markers,
                min_notice_interval: Duration::from_secs(value.min_notice_interval),
                state_dir: value.state_dir,
                landing_queue: Some(crate::landing_queue::path(&args.registry)),
                delivery_wait: Duration::from_secs(value.delivery_wait_minutes * 60),
            };
            let mut probe = crate::agent_watch::ManagedProbe::new(
                &client,
                &args.registry,
                &options.coordinator,
            );
            let mut notifier = crate::agent_watch::ManagedNotifier::new(&client, &args.registry);
            let report = crate::agent_watch::watch_agents(
                &mut probe,
                &mut notifier,
                &crate::agent_watch::SystemClock,
                &options,
            )?;
            write_json(&report).map_err(Failure::Output)?;
            return Ok(if report["ok"] == true { 0 } else { 1 });
        }
        Commands::Hold(value) => {
            let path = crate::landing_queue::path(&args.registry);
            let now = crate::agent_watch::Clock::now(&crate::agent_watch::SystemClock);
            let (queue, found) = match (value.list, value.release, value.name, value.after) {
                (true, ..) => (crate::landing_queue::load(&path)?, true),
                (false, true, Some(name), _) => {
                    let (found, queue) = crate::landing_queue::drop_hold(&path, &name, now)?;
                    (queue, found)
                }
                (false, false, Some(name), Some(after)) => (
                    crate::landing_queue::hold(&path, &name, &after, &value.note, now)?,
                    true,
                ),
                _ => {
                    return Err(Failure::Usage(
                        "hold needs NAME --after OTHER, NAME --release, or --list".to_owned(),
                    ))
                }
            };
            write_json(&queue).map_err(Failure::Output)?;
            return Ok(i32::from(!found));
        }
        Commands::InstallUnits(value) => {
            let absolute = |path: &Path| {
                std::path::absolute(path).map_err(|error| Failure::Usage(error.to_string()))
            };
            let herdr = if args.herdr_bin.components().count() == 1 {
                std::env::var_os("PATH")
                    .and_then(|path| {
                        std::env::split_paths(&path)
                            .map(|directory| directory.join(&args.herdr_bin))
                            .find(|candidate| candidate.is_file())
                    })
                    .ok_or_else(|| {
                        Failure::Usage(format!(
                            "{} is not on PATH; pass --herdr-bin with an absolute path",
                            args.herdr_bin.display()
                        ))
                    })?
            } else {
                absolute(&args.herdr_bin)?
            };
            let agentctl = match value.agentctl {
                Some(path) => absolute(&path)?,
                None => {
                    std::env::current_exe().map_err(|error| Failure::Usage(error.to_string()))?
                }
            };
            write_json(&crate::units::install(&crate::units::InstallOptions {
                dest: value.dest,
                agentctl,
                herdr,
                registry: absolute(&args.registry)?,
                bridge_unit: value.bridge_unit,
                dry_run: value.dry_run,
                force: value.force,
            })?)
            .map_err(Failure::Output)?;
            return Ok(0);
        }
        Commands::Profiles(value) => {
            let root = crate::profiles::configuration_root(&value.cwd, &args.registry, true)?;
            let (path, profiles, workspace) = crate::profiles::load_configuration(&root, true)?;
            write_json(&json!({
                "path": path,
                "workspace": workspace,
                "profiles": profiles.values().map(crate::profiles::LaunchProfile::public).collect::<Vec<_>>()
            }))
            .map_err(Failure::Output)?;
            return Ok(0);
        }
        Commands::Skill(value) => {
            if args.registry != Path::new(".agentctl")
                || args.herdr_bin != Path::new("herdr")
                || args.term_backend.is_some()
                || args.tmux_bin != Path::new("tmux")
                || args.tmux_socket.is_some()
                || args.agentcloudctl_bin != Path::new("agentcloudctl")
                || args.agentterm_bin != Path::new("agentterm")
                || args.from_session.is_some()
                || args.agentcloud_url.is_some()
            {
                return Err(Failure::Usage(
                    "--registry, --herdr-bin, --term-backend, --tmux-bin, --tmux-socket, --agentcloudctl-bin, --agentterm-bin, --agentcloud-url, and --from-session do not apply to skill install".to_owned(),
                ));
            }
            let SkillCommand::Install(install) = value.command;
            write_json(&crate::skill_install::install(
                &install.harness,
                install.force,
            )?)
            .map_err(Failure::Output)?;
            return Ok(0);
        }
        Commands::CodexGoal(value) => {
            if args.registry != Path::new(".agentctl")
                || args.herdr_bin != Path::new("herdr")
                || args.term_backend.is_some()
                || args.tmux_bin != Path::new("tmux")
                || args.tmux_socket.is_some()
                || args.agentcloudctl_bin != Path::new("agentcloudctl")
                || args.agentterm_bin != Path::new("agentterm")
                || args.from_session.is_some()
                || args.agentcloud_url.is_some()
            {
                return Err(Failure::Usage(
                    "--registry, --herdr-bin, --term-backend, --tmux-bin, --tmux-socket, --agentcloudctl-bin, --agentterm-bin, --agentcloud-url, and --from-session do not apply to codex-goal".to_owned(),
                ));
            }
            return crate::codex_goal::command::run(&value).map_err(Failure::Output);
        }
        _ => {}
    }
    let (selected_backend, backend_explicit) = crate::term::select_backend(
        args.term_backend.as_deref(),
        environment,
        &args.registry,
        &args.herdr_bin,
        &args.tmux_bin,
    )
    .map_err(AgentError::from)?;
    // Lifecycle runs on tmux only when an operator selected it (flag,
    // environment, or the registry's profile config). When detection
    // merely found no Herdr, fall through so the Herdr client reports its
    // usual not-found error, exactly as before backends were selectable.
    // A start `--backend` is also an operator selection for that start.
    let start_backend = match &command {
        // The flag's value parser admits only these two spellings.
        Commands::Start(start) => start.backend.as_deref().map(|backend| {
            if backend == "tmux" {
                term_backend::BackendKind::Tmux
            } else {
                term_backend::BackendKind::Herdr
            }
        }),
        _ => None,
    };
    let (selected_backend, backend_explicit) = match start_backend {
        Some(kind) if !backend_explicit => (kind, true),
        _ => (selected_backend, backend_explicit),
    };
    let client: Box<dyn crate::subagents::ManagedApi> = if backend_explicit
        && selected_backend == term_backend::BackendKind::Tmux
    {
        Box::new(term_backend::TmuxBackend::new(
            args.tmux_bin.clone(),
            args.tmux_socket.clone(),
        ))
    } else {
        Box::new(HerdrClient::with_executable("direct", &args.herdr_bin).map_err(AgentError::from)?)
    };
    let client: &dyn crate::subagents::ManagedApi = &*client;
    let from_environment = CloudTools::from_environment(environment);
    let manager = ManagedAgents::new(client, &args.registry)?.with_cloud_tools(CloudTools {
        agentcloudctl: args.agentcloudctl_bin,
        agentterm: args.agentterm_bin,
        caller_session: caller_session(args.from_session, from_environment.caller_session),
        endpoint_explicit: args.agentcloud_url.is_some(),
        endpoint: args.agentcloud_url.or(from_environment.endpoint),
        ambient_session: from_environment.ambient_session,
        hostname: from_environment.hostname,
    });
    let mut result = match command {
        Commands::Mcp => {
            return Ok(crate::mcp::serve(
                args.registry.to_string_lossy().as_ref(),
                args.herdr_bin.to_string_lossy().as_ref(),
            ));
        }
        Commands::Start(value) => {
            let cloud_flags = [
                (value.cloud_harness.is_some(), "--cloud-harness"),
                (value.provision, "--provision"),
                (value.envspec.is_some(), "--envspec"),
                (value.purpose.is_some(), "--purpose"),
                (value.cloud_workspace.is_some(), "--cloud-workspace"),
                (value.node_id.is_some(), "--node-id"),
                (value.cloud_title.is_some(), "--cloud-title"),
            ]
            .into_iter()
            .filter_map(|(present, flag)| present.then_some(flag))
            .collect::<Vec<_>>();
            let mut record_edges = !value.no_edges;
            let (harness, mode, model, reasoning_effort, harness_args, environment, cloud) =
                if let Some(profile_name) = value.profile.as_deref() {
                    let mut overlaps = Vec::new();
                    if value.mode.is_some() {
                        overlaps.push("--mode");
                    }
                    if value.harness.is_some() {
                        overlaps.push("--harness");
                    }
                    if value.model.is_some() {
                        overlaps.push("--model");
                    }
                    if value.reasoning_effort.is_some() {
                        overlaps.push("--reasoning-effort");
                    }
                    if !value.harness_args.is_empty() {
                        overlaps.push("--harness-arg");
                    }
                    if !value.environment.is_empty() {
                        overlaps.push("--env");
                    }
                    overlaps.extend_from_slice(&cloud_flags);
                    if !overlaps.is_empty() {
                        return Err(Failure::Usage(format!(
                            "--profile conflicts with explicit launch settings: {}",
                            overlaps.join(", ")
                        )));
                    }
                    let root =
                        crate::profiles::configuration_root(&value.cwd, &args.registry, false)?;
                    let (_, profiles) = crate::profiles::load_profiles(&root, false)?;
                    let profile = profiles.get(profile_name).ok_or_else(|| {
                        Failure::Usage(format!(
                            "unknown profile {profile_name:?}; run agentctl profiles --cwd {}",
                            value.cwd.display()
                        ))
                    })?;
                    if profile.record_edges == Some(false) {
                        record_edges = false;
                    }
                    (
                        profile.harness.clone(),
                        profile.mode.clone(),
                        profile.model.clone(),
                        profile.reasoning_effort.clone(),
                        profile.argv.clone(),
                        profile.environment.clone(),
                        profile.cloud.clone(),
                    )
                } else {
                    let harness = value.harness.unwrap_or_else(|| "codex".to_owned());
                    let cloud = if harness == "agentcloud" {
                        Some(CloudLaunch {
                            harness: value.cloud_harness,
                            provision: value.provision,
                            envspec: value.envspec,
                            purpose: value.purpose,
                            workspace: value.cloud_workspace,
                            node_id: value.node_id,
                            title: value.cloud_title,
                        })
                    } else {
                        if !cloud_flags.is_empty() {
                            return Err(Failure::Usage(format!(
                                "{} apply only with --harness agentcloud",
                                cloud_flags.join(", ")
                            )));
                        }
                        crate::profiles::validate_raw_harness_arguments(
                            "launch",
                            &harness,
                            &value.harness_args,
                            value.model.is_some(),
                            value.reasoning_effort.is_some(),
                            value.resume.is_some(),
                        )?;
                        None
                    };
                    (
                        harness,
                        value.mode.unwrap_or_else(|| "interactive".to_owned()),
                        value.model,
                        value.reasoning_effort,
                        value.harness_args,
                        value.environment,
                        cloud,
                    )
                };
            if mode != "interactive" {
                return Err(Failure::Usage("Rust agentctl supports interactive sessions; use agentctl with the worker extension for headless workers (the Python edition; for example DAGRUN_ENGINE=python)".to_owned()));
            }
            if value
                .backend
                .as_deref()
                .is_some_and(|backend| backend != selected_backend.as_str())
            {
                return Err(Failure::Usage(format!(
                    "start --backend {} does not match the selected terminal backend {}; \
                     pass a matching --term-backend or drop --backend",
                    value.backend.as_deref().unwrap_or_default(),
                    selected_backend.as_str()
                )));
            }
            let brief = match value.file {
                Some(path) => Some(
                    fs::read_to_string(path).map_err(|error| Failure::Usage(error.to_string()))?,
                ),
                None => value.brief,
            };
            let slot = if value.project_box {
                Some(SlotLaunch {
                    slot: value
                        .box_name
                        .clone()
                        .unwrap_or_else(|| value.agent.name.clone()),
                    isolation: value.box_isolation.clone(),
                    project: value.box_project.clone(),
                    project_box: true,
                    box_writable: value.box_writable.clone(),
                    ..SlotLaunch::default()
                })
            } else {
                value.slot.clone().map(|slot| SlotLaunch {
                    slot,
                    isolation: value.slot_isolation.clone(),
                    project: value.slot_project.clone(),
                    ..SlotLaunch::default()
                })
            };
            let options = StartOptions {
                workspace_id: value.workspace_id,
                profile: value.profile,
                harness,
                model,
                resume: value.resume,
                harness_args,
                environment,
                brief,
                startup_timeout: Duration::from_secs_f64(value.startup_timeout),
                delivery: value.delivery.options(),
                cloud,
                slot,
                record_edges,
            };
            if let Some(effort) = reasoning_effort.as_deref() {
                manager.start_with_reasoning_effort(
                    &value.agent.name,
                    &value.cwd,
                    effort,
                    options,
                )?
            } else {
                manager.start(&value.agent.name, &value.cwd, options)?
            }
        }
        Commands::Adopt(value) => {
            let mut result = manager.adopt(
                &value.agent.name,
                AdoptOptions {
                    pane_id: value.pane,
                    expected_workspace: value.workspace,
                    cwd: value.cwd,
                    harness: value.harness,
                    session: value.session,
                    record_edges: !value.no_edges,
                },
            )?;
            manager.designate_after(&mut result, &value.agent.name, value.as_coordinator);
            result
        }
        Commands::Coordinator(value) => match value.command {
            CoordinatorCommand::Set(agent) => manager.coordinator_set(&agent.name)?,
            CoordinatorCommand::Show => {
                let shown = manager.coordinator_show()?;
                write_json(&shown).map_err(Failure::Output)?;
                return Ok(if shown["designated"] == true { 0 } else { 1 });
            }
            CoordinatorCommand::Clear => manager.coordinator_clear()?,
        },
        Commands::Manage(value) => manager.manage(
            &value.agent.name,
            match (value.by, value.clear) {
                (Some(manager), _) => ManageAction::By(manager),
                (None, true) => ManageAction::Clear,
                (None, false) => ManageAction::Caller,
            },
        )?,
        Commands::Tree(value) => {
            let tree = manager.tree()?;
            if value.json {
                write_json(&tree).map_err(Failure::Output)?;
            } else {
                print!("{}", crate::subagents::render_tree(&tree));
            }
            return Ok(0);
        }
        Commands::Move(value) => manager.move_to_project_workspace(&value.name)?,
        Commands::Rename(value) => manager.rename(&value.agent.name, &value.new_name)?,
        Commands::Promote(value) => {
            manager.promote(&value.name, value.expected_token.as_deref())?
        }
        Commands::Anchor(value) => manager.anchor(
            &value.agent.name,
            value.replace,
            value.session_id.as_deref(),
        )?,
        Commands::Doctor(value) => {
            let report = manager.doctor(value.repair_labels)?;
            write_json(&report).map_err(Failure::Output)?;
            return Ok(if report["clean"] == true { 0 } else { 1 });
        }
        Commands::Stop(value) => {
            let recovery_selector = if value.retire_dead_adoption {
                Some("--retire-dead-adoption")
            } else if value.recover_legacy_adoption {
                Some("--recover-legacy-adoption")
            } else {
                None
            };
            if let Some(selector) = recovery_selector.filter(|_| {
                value.expected_token.is_none() || value.expected_record_sha256.is_none()
            }) {
                return Err(Failure::Usage(format!(
                    "{selector} requires --expected-token and --expected-record-sha256"
                )));
            }
            if recovery_selector.is_none() && value.expected_record_sha256.is_some() {
                return Err(Failure::Usage(
                    "--expected-record-sha256 requires --recover-legacy-adoption or --retire-dead-adoption".to_owned(),
                ));
            }
            manager.advised_stop_with_options(
                &value.agent.name,
                StopOptions {
                    expected_token: value.expected_token,
                    recover_legacy_adoption: value.recover_legacy_adoption,
                    retire_dead_adoption: value.retire_dead_adoption,
                    expected_record_sha256: value.expected_record_sha256,
                    skip_cloud_halt: value.skip_cloud_halt,
                },
            )?
        }
        Commands::Revive(value) => {
            let timeout = Duration::from_secs_f64(value.startup_timeout);
            let mut result = if value.all {
                manager.revive_all(value.dry_run, timeout)?
            } else {
                manager.revive(
                    value.name.as_deref().expect("clap requires NAME or --all"),
                    value.dry_run,
                    value.expected_token.as_deref(),
                    timeout,
                )?
            };
            add_capabilities(&mut result);
            if let Some(agents) = result.get_mut("agents") {
                add_capabilities(agents);
            }
            let code =
                if !value.dry_run && result["blocked"].as_u64().is_some_and(|count| count > 0) {
                    75
                } else {
                    0
                };
            write_json(&result).map_err(Failure::Output)?;
            return Ok(code);
        }
        Commands::List => json!(manager.list()?),
        Commands::Status(value) => manager.status(&value.name)?,
        Commands::Prompts(value) => manager.prompts(&value.name, None)?,
        Commands::Answer(value) => manager.prompts(
            &value.agent.name,
            Some((value.option, value.text.as_deref())),
        )?,
        Commands::Send(value) => {
            if value.model.is_some() {
                return Err(Failure::Usage(
                    "--model on send requires a headless worker and an installation with the worker extension (the Python edition; for example DAGRUN_ENGINE=python)"
                        .to_owned(),
                ));
            }
            let text = value
                .prompt
                .read()
                .map_err(Failure::Usage)?
                .ok_or_else(|| Failure::Usage("send requires prompt text or --file".to_owned()))?;
            if manager.is_cloud(&value.agent.name)? {
                if value.ttl.is_some() {
                    return Err(Failure::Usage(
                        "--ttl applies to interactive agents; agentcloud journals the prompt at once"
                            .to_owned(),
                    ));
                }
                manager.send_cloud(&value.agent.name, &text, value.message_id.as_deref())?
            } else {
                let not_after = value.ttl.map(|ttl| {
                    crate::agent::now_millis().saturating_add((ttl * 1000.0).ceil() as u64)
                });
                json!(manager.send_identified(
                    &value.agent.name,
                    &text,
                    DrainOptions {
                        not_after,
                        ..value.delivery.options()
                    },
                    value.message_id.as_deref()
                )?)
            }
        }
        Commands::Cancel(value) => {
            manager.cancel_message(&value.agent.name, &value.message_id, &value.reason)?
        }
        Commands::Drain(value) => {
            let options = DrainOptions {
                deadline: value.deadline.map(Duration::from_secs_f64),
                lock_timeout: value.lock_timeout.map(Duration::from_secs_f64),
                max_items: value
                    .max_items
                    .map(|limit| usize::try_from(limit).unwrap_or(usize::MAX)),
                ..value.delivery.options()
            };
            let result = manager.drain(&value.agent.name, options)?;
            write_json(&result).map_err(Failure::Output)?;
            return Ok(if result.blocked.is_some() {
                75
            } else if result.quarantined.is_empty() {
                0
            } else {
                76
            });
        }
        Commands::Read(value) => {
            if manager.is_cloud(&value.agent.name)? {
                if value.since_turn.is_some() || value.output == "since_turn" {
                    return Err(Failure::Usage("agentcloud agents support --output last, tail, or all; since_turn requires a headless transcript".to_owned()));
                }
                print!(
                    "{}",
                    manager.read_cloud(&value.agent.name, value.lines as usize, &value.output)?
                );
                return Ok(0);
            }
            if !matches!(value.output.as_str(), "tail" | "all") || value.since_turn.is_some() {
                return Err(Failure::Usage("interactive agents expose terminal snapshots; last/since_turn require headless transcripts (read a headless worker with the Python edition; for example DAGRUN_ENGINE=python)".to_owned()));
            }
            print!("{}", manager.read(&value.agent.name, value.lines as usize)?);
            return Ok(0);
        }
        Commands::Wait(value) => {
            manager.wait(&value.agent.name, Duration::from_secs_f64(value.timeout))?
        }
        Commands::Goal(value) => manager.goal(
            &value.agent.name,
            value.prompt.read().map_err(Failure::Usage)?.as_deref(),
            value.delivery.options(),
            value
                .goal_command_json
                .as_ref()
                .map(|command| command.0.as_slice()),
        )?,
        Commands::BindSession(value) => manager.bind_session(
            &value.agent.name,
            &value.session,
            value
                .goal_command_json
                .as_ref()
                .map(|command| command.0.as_slice()),
        )?,
        Commands::Attach(value) => manager.attach(&value.name)?,
        Commands::AttachSelf(value) => {
            let mut result = manager.attach_self(
                &value.agent.name,
                &value.cwd,
                StartOptions {
                    workspace_id: value.workspace_id,
                    harness: "agentcloud".to_owned(),
                    startup_timeout: Duration::from_secs_f64(value.startup_timeout),
                    // The session registers itself; it is nobody's worker.
                    record_edges: false,
                    ..StartOptions::default()
                },
            )?;
            manager.designate_after(&mut result, &value.agent.name, value.as_coordinator);
            result
        }
        Commands::Reset(value) => manager.reset(&value.name)?,
        Commands::Pause(value) => manager.pause(&value.name, true)?,
        Commands::Resume(value) => manager.pause(&value.name, false)?,
        Commands::Capabilities
        | Commands::Quickstart
        | Commands::Userguide
        | Commands::Chat(_)
        | Commands::Inbox(_)
        | Commands::Profiles(_)
        | Commands::Skill(_)
        | Commands::CodexGoal(_)
        | Commands::Restore(_)
        | Commands::InstallUnits(_)
        | Commands::Watch(_)
        | Commands::WatchAgents(_)
        | Commands::Hold(_) => {
            unreachable!()
        }
    };
    add_capabilities(&mut result);
    write_json(&result).map_err(Failure::Output)?;
    // `list` prints every row it could read; an unreadable row makes the listing incomplete.
    let incomplete = result.as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row.get("record_error").and_then(Value::as_bool) == Some(true))
    });
    Ok(i32::from(incomplete))
}

/// Make `config` follow the registry's coordinator designation, or say why it cannot. A
/// designation that cannot be read, or that disagrees with `restore.json`, never stops `restore`
/// or `watch`: they go on with `restore.json`'s own coordinator and report the problem.
fn designation_problem(
    client: &HerdrClient,
    registry: &Path,
    config: &mut crate::restore::RestoreConfig,
) -> Option<String> {
    let designated = match ManagedAgents::new(client, registry)
        .and_then(|manager| manager.designated_coordinator())
    {
        Ok(designated) => designated,
        Err(error) => {
            return Some(format!(
                "the coordinator designation is unreadable: {error}"
            ))
        }
    };
    config
        .follow_designation(designated)
        .err()
        .map(|error| error.to_string())
}

/// The agentcloud session input is sent from: `--from-session`, else `$AGENTCLOUD_SESSION_ID`.
/// An explicit empty value selects human attribution; the send path refuses it when the process
/// runs inside a different agentcloud session than the recipient's.
fn caller_session(flag: Option<String>, environment: Option<String>) -> Option<String> {
    match flag {
        Some(session) if session.is_empty() => None,
        Some(session) => Some(session),
        None => environment.filter(|session| !session.is_empty()),
    }
}

fn run_chat(registry: PathBuf, herdr_bin: PathBuf, chat: Chat) -> Result<i32, Failure> {
    match chat.command {
        ChatCommand::Quickstart => {
            print!("{}", crate::CHAT_QUICKSTART);
            Ok(0)
        }
        ChatCommand::Userguide => {
            print!("{}", crate::CHAT_USER_GUIDE);
            Ok(0)
        }
        ChatCommand::Status(value) => {
            let mut result =
                crate::chat_service::status(&value.bridge_state).map_err(Failure::Chat)?;
            with_agent_lock(&mut result, &registry);
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Grant(value) => {
            let result = crate::chat_service::grant(
                &value.state.bridge_state,
                &value.senders,
                value.all_members,
                value.duration,
                value.note.as_deref(),
            )
            .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Grants(value) => {
            let listing = crate::chat_service::grants(&value.state.bridge_state, value.json)
                .map_err(Failure::Chat)?;
            write_text(&listing).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Revoke(value) => {
            let result = crate::chat_service::revoke(&value.state.bridge_state, &value.senders)
                .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::GrantLimit(value) => {
            let result =
                crate::chat_service::grant_limit(&value.state.bridge_state, value.duration)
                    .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::RetryCheckpointGap(value) => {
            let result = crate::chat_service::retry_checkpoint_gap(
                &value.state.bridge_state,
                &crate::chat_runtime::CheckpointGapRetryApproval {
                    expected_gap_sha256: &value.expected_gap_sha256,
                    expected_checkpoint_sha256: &value.expected_checkpoint_sha256,
                    expected_configuration_sha256: &value.expected_configuration_sha256,
                    keep_cursor: &value.keep_cursor,
                    evidence_path: &value.evidence_file,
                    evidence_sha256: &value.evidence_sha256,
                },
            )
            .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::RetryBoundaryGap(value) => {
            let result = crate::chat_service::retry_boundary_gap(
                &value.state.bridge_state,
                &crate::chat_runtime::GapRetryApproval {
                    expected_gap_sha256: &value.expected_gap_sha256,
                    expected_checkpoint_sha256: &value.expected_checkpoint_sha256,
                    expected_configuration_sha256: &value.expected_configuration_sha256,
                    keep_cursor: &value.keep_cursor,
                    evidence_path: &value.evidence_file,
                    evidence_sha256: &value.evidence_sha256,
                },
            )
            .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::AcceptCheckpointBoundary(value) => {
            let result = crate::chat_service::accept_checkpoint_boundary(
                &value.state.bridge_state,
                &crate::chat_runtime::CheckpointBoundaryAcceptance {
                    expected_checkpoint_sha256: &value.expected_checkpoint_sha256,
                    expected_configuration_sha256: &value.expected_configuration_sha256,
                    keep_cursor: &value.keep_cursor,
                },
            )
            .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Inspect(value) => {
            let result =
                crate::chat_service::inspect_request(&value.state.bridge_state, &value.request)
                    .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Thread(value) => {
            let history = crate::chat_service::thread_history(
                &value.state.bridge_state,
                &value.thread,
                value.last,
            )
            .map_err(Failure::Chat)?;
            write_text(&history).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Sent(value) => {
            let listing = crate::chat_service::sent_replies(
                &value.state.bridge_state,
                value.request.as_deref(),
                value.last,
                value.json,
            )
            .map_err(Failure::Chat)?;
            write_text(&listing).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Publish(value) => {
            let body = value.read_message().map_err(Failure::Usage)?;
            let result = crate::chat_service::publish(
                &value.state.bridge_state,
                &value.channel_id,
                value.thread_id.as_deref(),
                &value.request_id,
                &body,
                value.slack_format,
            )
            .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Reply(value) => {
            let body = value.read_reply()?;
            match crate::chat_service::reply(
                &value.state.bridge_state,
                &value.request,
                &value.reply_id,
                &body,
            )
            .map_err(Failure::Chat)?
            {
                crate::chat_service::ReplyCommandOutcome::Stored(result) => {
                    write_json(&result).map_err(Failure::Output)?;
                    Ok(0)
                }
                crate::chat_service::ReplyCommandOutcome::TryAgain(reason) => {
                    eprintln!("agentctl: {reason}");
                    Ok(75)
                }
            }
        }
        ChatCommand::Close(value) => {
            let result = crate::chat_service::close(&value.state.bridge_state, &value.request)
                .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Context(value) => {
            let output = crate::chat_context::read_context(
                &value.state.bridge_state,
                &value.request,
                value.limit,
                value.cursor.as_deref(),
            )
            .map_err(|error| {
                Failure::Chat(crate::chat_runtime::ChatRuntimeError::Invalid(error).into())
            })?;
            write_text(&format!("{output}\n")).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::History(value) => {
            let output =
                crate::chat_history::read_reply_history(&value.state.bridge_state, &value.request)
                    .map_err(|error| {
                        Failure::Chat(crate::chat_runtime::ChatRuntimeError::Invalid(error).into())
                    })?;
            write_text(&format!("{output}\n")).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Init(value) => {
            let client =
                HerdrClient::with_executable("direct", &herdr_bin).map_err(AgentError::from)?;
            let manager = ManagedAgents::new(&client, &registry)?;
            let result =
                crate::chat_service::initialize(&value.state.bridge_state, &value.config, &manager)
                    .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::Tick(value) => {
            let options = value.options();
            let client =
                HerdrClient::with_executable("direct", &herdr_bin).map_err(AgentError::from)?;
            // A person's messages go ahead of agent-to-agent sends (#556 chat-delivery-latency).
            let manager = ManagedAgents::new(&client, &registry)?.with_lock_priority();
            let result = crate::chat_service::tick(&value.state.bridge_state, &manager, options)
                .map_err(Failure::Chat)?;
            let code = if result.has_errors() { 75 } else { 0 };
            write_json(&result).map_err(Failure::Output)?;
            Ok(code)
        }
        ChatCommand::Run(value) => {
            let mut options = value.operate.options();
            options.delivery.deliver_while_working = value.deliver_while_working;
            let client =
                HerdrClient::with_executable("direct", &herdr_bin).map_err(AgentError::from)?;
            // A person's messages go ahead of agent-to-agent sends (#556 chat-delivery-latency).
            let manager = ManagedAgents::new(&client, &registry)?.with_lock_priority();
            let result = crate::chat_service::run_with_settings(
                &value.operate.state.bridge_state,
                &client,
                &manager,
                options,
                crate::chat_service::RunSettings {
                    ignored_text_prefixes: value.ignored_text_prefixes,
                    offer_reply_command: value.offer_reply_command,
                    stale_composer_clear_after: value.stale_composer_clear_after,
                    reply_reminders: value.reply_reminder_idle.0.map(|idle| {
                        crate::chat_service::ReplyReminderSettings {
                            idle,
                            claude_bin: value.claude_bin.clone(),
                        }
                    }),
                },
            )
            .map_err(Failure::Chat)?;
            write_json(&result).map_err(Failure::Output)?;
            Ok(0)
        }
        ChatCommand::GracefulStopMain(value) => {
            crate::chat_service::graceful_stop_main(value.main_pid, value.main_pidfd_id)
                .map_err(Failure::Chat)?;
            Ok(0)
        }
    }
}

fn capabilities_document(
    registry: PathBuf,
    plugins: crate::plugins::PluginInventory,
    selected_backend: &str,
) -> serde_json::Value {
    let not_live = crate::plugins::MaturityStatus::discovered(false);
    json!({
        "interactive": {
            "backends": ["herdr"],
            "harnesses": ["codex", "claude", "muse"]
        },
        "term_backends": {
            "available": ["herdr", "tmux"],
            "selected": selected_backend
        },
        "agentcloud": {
            "drivers": crate::subagents::CLOUD_DRIVERS,
            "requires": ["herdr", "agentcloudctl", "agentterm"],
            "capabilities": CLOUD_CAPABILITIES
        },
        "headless": null,
        "queue": {
            "lanes": [
                agent_queue::queue::INBOX_V2,
                agent_queue::queue::CANCEL,
                agent_queue::queue::CANCELLED,
            ],
            "drain_bounds": ["deadline", "lock_timeout", "max_items"],
        },
        "build": crate::build_info::document(),
        "services": [{
            "name": "chat-bridge",
            "origin": "built_in",
            "status": crate::plugins::MaturityStatus::discovered(true)
        }],
        "registry": registry,
        "chat_subscriptions": {
            "core": {
                "origin": "built_in",
                "protocol_name": chat_subscription_plugin::PROTOCOL_NAME,
                "protocol_version": chat_subscription_plugin::PROTOCOL_VERSION,
                "max_frame_bytes": chat_subscription_plugin::MAX_FRAME_BYTES,
                "status": crate::plugins::MaturityStatus::discovered(true)
            },
            "reference_backends": [
                {
                    "name": "google-workspace-events",
                    "origin": "reference",
                    "status": not_live,
                    "available": ["design"],
                    "missing": [
                        "rust-provider",
                        "cloud-topic",
                        "pull-subscription",
                        "application-default-credentials",
                        "live-end-to-end-verification"
                    ]
                },
                {
                    "name": "discord-gateway",
                    "origin": "reference",
                    "status": crate::plugins::MaturityStatus::discovered(false),
                    "available": ["request-response-client"],
                    "missing": ["gateway-subscription", "live-end-to-end-verification"]
                }
            ],
            "plugins": plugins
        }
    })
}

/// Named operations an agentcloud-backed agent supports.
const CLOUD_CAPABILITIES: [&str; 9] = [
    "send",
    "status",
    "read",
    "wait",
    "stop",
    "attach",
    "pause",
    "resume",
    "final-output",
];

fn add_capabilities(value: &mut serde_json::Value) {
    if let Some(values) = value.as_array_mut() {
        for value in values {
            add_capabilities(value);
        }
    } else if value.get("name").is_some()
        && (value.get("adapter").is_some() || value.get("launch").is_some_and(Value::is_object))
    {
        // A nested agentctl-session/v2 or v3 row keeps its launch fields under `launch`.
        let launch = if value.get("adapter").is_some() {
            &*value
        } else {
            &value["launch"]
        };
        let interactive = launch["mode"] == "interactive"
            && (launch["backend"] == "herdr" || launch["backend"] == "tmux");
        let adapter = launch["adapter"].as_str().map(str::to_owned);
        let adapter = adapter.as_deref();
        value["capabilities"] = if adapter == Some("agentcloud") {
            json!(CLOUD_CAPABILITIES)
        } else if matches!(
            adapter,
            Some(
                "herdr" | "herdr-pane" | "herdr-foreign" | "herdr-relay" | "tmux" | "tmux-foreign"
            )
        ) && interactive
        {
            let mut capabilities = vec![
                "send",
                "status",
                "read",
                "wait",
                "stop",
                "attach",
                "pause",
                "resume",
                "terminal-snapshot",
                "drain",
                "goal",
                "bind-session",
            ];
            if adapter == Some("herdr-relay") {
                // Goals are slash commands, which a relayed pane cannot take.
                capabilities.retain(|capability| *capability != "goal");
            }
            if adapter == Some("herdr") {
                capabilities.push("move");
            }
            if matches!(
                adapter,
                Some("herdr" | "herdr-foreign" | "tmux" | "tmux-foreign")
            ) {
                capabilities.extend(["anchor", "rename"]);
            } else if matches!(adapter, Some("herdr-pane" | "herdr-relay")) {
                // A custom pane's anchor only records the launch spec of its pinned process.
                capabilities.push("anchor");
            }
            // An adopted agent becomes revivable once a launch spec records what it runs.
            // On tmux there is also no runtime to report a conversation later,
            // so revive additionally needs one already recorded.
            let tmux_conversation =
                launch["backend"] != "tmux" || value["native_session"].is_object();
            if tmux_conversation
                && (matches!(
                    adapter,
                    Some("herdr" | "herdr-pane" | "herdr-relay" | "tmux")
                ) || (matches!(adapter, Some("herdr-foreign" | "tmux-foreign"))
                    && value["launch_spec"].is_object()))
            {
                capabilities.push("revive");
            }
            json!(capabilities)
        } else {
            json!(["status"])
        };
    }
}

/// Add to a `chat status` report who holds the bridged agent's lifecycle lock in `registry` and
/// who is waiting for it, as `delivery_alarm.agent_lock`, and give each stalled request that
/// has not reached the agent's queue the reason it is still waiting, as `waiting_on`. A request
/// waits for that lock behind every other sender to the agent, which the bridge's own records
/// cannot show (#556 chat-delivery-latency).
fn with_agent_lock(result: &mut Value, registry: &Path) {
    let Some(agent) = result["agent_name"].as_str().map(str::to_owned) else {
        return;
    };
    let report = crate::lock_wait::contention(registry, &agent);
    let waiting_on = crate::lock_wait::describe(&agent, &report);
    let Some(alarm) = result
        .get_mut("delivery_alarm")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if let (Some(sentence), Some(stalled)) = (
        waiting_on,
        alarm.get_mut("stalled").and_then(Value::as_array_mut),
    ) {
        for request in stalled
            .iter_mut()
            .filter(|request| matches!(request["phase"].as_str(), Some("pending" | "submitting")))
        {
            request["waiting_on"] = Value::String(sentence.clone());
        }
    }
    alarm.insert("agent_lock".to_owned(), report);
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent;
    use clap::CommandFactory;

    fn profile_resume_arguments(root: &Path) -> Vec<OsString> {
        vec![
            "agentctl".into(),
            "--registry".into(),
            root.join("registry").into_os_string(),
            "--herdr-bin=/definitely/missing/herdr".into(),
            "start".into(),
            "worker".into(),
            "--cwd".into(),
            root.as_os_str().to_owned(),
            "--profile=reviewer".into(),
            "--resume=saved-conversation".into(),
        ]
    }

    #[test]
    fn chat_status_names_the_lock_holder_and_waiters_for_stalled_requests() {
        let registry = std::env::temp_dir().join(format!(
            "agentctl-cli-agent-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::agent::create_private_directory(&registry, "registry", true, true).unwrap();
        let lock = crate::agent::open_private_lock(
            &crate::lock_wait::lock_path(&registry, "coordinator"),
            "lock",
        )
        .unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
        let _waiting = crate::lock_wait::register(&registry, "coordinator", true).unwrap();
        let mut status = json!({
            "agent_name": "coordinator",
            "delivery_alarm": {"stalled": [
                {"key": "a", "phase": "submitting"},
                {"key": "b", "phase": "delivery_uncertain"},
            ]},
        });
        with_agent_lock(&mut status, &registry);
        let alarm = &status["delivery_alarm"];
        assert_eq!(
            alarm["agent_lock"]["holder"]["pid"],
            json!(std::process::id())
        );
        assert_eq!(alarm["agent_lock"]["waiters"][0]["priority"], json!(true));
        let waiting_on = alarm["stalled"][0]["waiting_on"].as_str().unwrap();
        assert!(waiting_on.starts_with("1 sender waiting for agent coordinator's"));
        assert!(alarm["stalled"][1].get("waiting_on").is_none());
        let _ = fs::remove_dir_all(&registry);
    }
    #[test]
    fn python_only_commands_are_detected_behind_global_options() {
        let cases: &[(&[&str], Option<&str>)] = &[
            (&["agentctl", "mcp"], None),
            (
                &["agentctl", "--registry", "/tmp/r", "migrate"],
                Some("migrate"),
            ),
            (
                &["agentctl", "--herdr-bin=/bin/herdr", "reset"],
                None,
            ),
            (
                &["agentctl", "chat", "launch", "--config", "c.json"],
                Some("chat launch"),
            ),
            (&["agentctl", "--state=/tmp/r", "chat", "history"], None),
            (&["agentctl", "chat", "context"], None),
            (&["agentctl", "--", "mcp"], None),
            (&["agentctl", "chat", "--", "launch"], Some("chat launch")),
            (&["agentctl", "help", "mcp"], None),
            (&["agentctl", "help", "chat", "launch"], Some("chat launch")),
            (&["agentctl", "chat", "help", "launch"], Some("chat launch")),
            (&["agentctl", "chat", "status"], None),
            (&["agentctl", "chat", "--help"], None),
            (&["agentctl", "chat", "--help", "launch"], None),
            (&["agentctl", "--help", "mcp"], None),
            (&["agentctl", "--version", "mcp"], None),
            (&["agentctl", "help", "list"], None),
            (&["agentctl", "help", "chat", "status"], None),
            (&["agentctl", "list"], None),
            (&["agentctl", "--registry", "mcp", "list"], None),
            (&["agentctl", "send", "reviewer", "mcp"], None),
        ];
        for (argv, expected) in cases {
            let arguments: Vec<OsString> = argv.iter().map(OsString::from).collect();
            assert_eq!(
                python_only_command(&arguments).as_deref(),
                *expected,
                "{argv:?}"
            );
        }
    }

    #[test]
    fn profile_resume_cli_accepts_the_combination_and_documents_its_local_scope() {
        let fixture = crate::subagents::tests::Fixture::new();
        let parsed = Cli::try_parse_from(profile_resume_arguments(&fixture.root)).unwrap();
        let Some(Commands::Start(start)) = parsed.command else {
            panic!("start command");
        };
        assert_eq!(start.profile.as_deref(), Some("reviewer"));
        assert_eq!(start.resume.as_deref(), Some("saved-conversation"));
        let help = Cli::try_parse_from(["agentctl", "start", "--help"])
            .err()
            .unwrap()
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(help.contains("local interactive Claude, Codex, or Muse"));
        assert!(help.contains("--profile reviewer --resume CONVERSATION_ID"));
        assert!(help.contains("other explicit launch settings conflict"));
    }

    #[test]
    fn profile_resume_cli_retains_every_other_explicit_profile_override_conflict() {
        let fixture = crate::subagents::tests::Fixture::new();
        for (flag, arguments) in [
            ("--mode", vec!["--mode=interactive"]),
            ("--harness", vec!["--harness=codex"]),
            ("--model", vec!["--model=other-model"]),
            ("--reasoning-effort", vec!["--reasoning-effort=high"]),
            ("--harness-arg", vec!["--harness-arg=--other-policy"]),
            ("--env", vec!["--env=REVIEW_MODE=other"]),
            ("--cloud-harness", vec!["--cloud-harness=native"]),
            ("--provision", vec!["--provision"]),
            ("--envspec", vec!["--provision", "--envspec=example"]),
            ("--purpose", vec!["--provision", "--purpose=example"]),
            ("--cloud-workspace", vec!["--cloud-workspace=/work/example"]),
            ("--node-id", vec!["--node-id=example"]),
        ] {
            let mut argv = profile_resume_arguments(&fixture.root);
            argv.extend(arguments.iter().map(OsString::from));
            let error = run(Cli::try_parse_from(argv).unwrap(), &|_| None).unwrap_err();
            let Failure::Usage(message) = error else {
                panic!("expected usage refusal for {flag}");
            };
            assert!(message.contains("--profile conflicts"), "{flag}: {message}");
            assert!(message.contains(flag), "{flag}: {message}");
            assert!(!message.contains("--resume"), "{flag}: {message}");
        }
        assert!(!fixture.root.join("registry").exists());
    }

    #[test]
    fn profile_resume_cli_keeps_headless_and_agentcloud_out_of_native_resume_scope() {
        for (harness, mode) in [("codex", "headless"), ("agentcloud", "interactive")] {
            let fixture = crate::subagents::tests::Fixture::new();
            assert!(std::process::Command::new("/usr/bin/git")
                .args(["init", "-q"])
                .arg(&fixture.root)
                .status()
                .unwrap()
                .success());
            fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
            let directory = fixture.root.join(".agentctl");
            agent::create_private_directory(&directory, "test launch profiles", false, false)
                .unwrap();
            agent::atomic_json(
                &directory.join("profiles.json"),
                &json!({
                    "schema": "agentctl-profiles/v1",
                    "profiles": {"reviewer": {"harness": harness, "mode": mode}},
                }),
            )
            .unwrap();
            let error = run(
                Cli::try_parse_from(profile_resume_arguments(&fixture.root)).unwrap(),
                &|_| None,
            )
            .unwrap_err();
            if mode == "headless" {
                let Failure::Usage(message) = error else {
                    panic!("expected headless usage refusal");
                };
                assert!(message.contains("headless"));
            } else {
                let Failure::Agent(error) = error else {
                    panic!("expected agentcloud resume refusal");
                };
                assert_eq!(error.exit_code(), 75);
                assert!(error
                    .to_string()
                    .contains("--resume does not apply to agentcloud"));
            }
            assert!(!fixture.root.join("registry").exists());
        }
    }

    #[test]
    fn stop_advice_parser_uses_recognized_grammar_and_successfully_parsed_globals() {
        let prefix = [
            "agentctl",
            "--registry=/work/record dir/'registry'",
            "--herdr-bin=-literal herdr $argument",
        ];
        let expected = StopContext::new(
            Path::new("/work/record dir/'registry'"),
            Path::new("-literal herdr $argument"),
        )
        .command(&RecoveryAction::Doctor);
        for suffix in [
            &["stop"][..],
            &["stop", "worker", "--unknown-option"][..],
            &["stop", "worker", "--expected-token"][..],
        ] {
            for arguments in [
                [&prefix[..], suffix].concat(),
                [&prefix[..1], &suffix[..1], &prefix[1..], &suffix[1..]].concat(),
                [
                    &[
                        "agentctl",
                        "--registry=/work/earlier",
                        "--herdr-bin=earlier",
                    ][..],
                    &suffix[..1],
                    &prefix[1..],
                    &suffix[1..],
                ]
                .concat(),
            ] {
                let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
                assert_eq!(
                    Cli::try_parse_from(arguments.iter().cloned())
                        .err()
                        .unwrap()
                        .exit_code(),
                    2
                );
                assert_eq!(
                    partial_stop_context(&arguments)
                        .expect("grammar identified stop before the error")
                        .command(&RecoveryAction::Doctor),
                    expected
                );
            }
        }
        for arguments in [
            vec!["agentctl", "--registry", "stop"],
            vec!["agentctl", "--herdr-bin=stop", "status"],
            vec!["agentctl", "send", "worker", "stop", "--unknown-option"],
        ] {
            let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
            assert!(partial_stop_context(&arguments).is_none());
        }
    }

    #[test]
    fn stop_advice_bound_flags_accept_leading_hyphen_tokens_as_one_argument() {
        let parsed = Cli::try_parse_from([
            "agentctl",
            "stop",
            "worker",
            "--expected-token=-original-generation",
            "--skip-cloud-halt",
        ])
        .unwrap();
        let Some(Commands::Stop(options)) = parsed.command else {
            panic!("stop command");
        };
        assert_eq!(
            options.expected_token.as_deref(),
            Some("-original-generation")
        );
        assert!(options.skip_cloud_halt);
    }

    #[test]
    fn revive_requires_one_scope_and_limits_startup_deadlines() {
        for arguments in [
            vec!["agentctl", "revive"],
            vec!["agentctl", "revive", "worker", "--all"],
            vec![
                "agentctl",
                "revive",
                "--all",
                "--expected-token",
                "generation",
            ],
            vec!["agentctl", "revive", "worker", "--startup-timeout", "0"],
            vec!["agentctl", "revive", "worker", "--startup-timeout", "301"],
            vec!["agentctl", "revive", "worker", "--startup-timeout", "NaN"],
        ] {
            assert_eq!(Cli::try_parse_from(arguments).err().unwrap().exit_code(), 2);
        }
        let parsed = Cli::try_parse_from([
            "agentctl",
            "revive",
            "worker",
            "--expected-token",
            "generation",
            "--dry-run",
        ])
        .unwrap();
        let Some(Commands::Revive(value)) = parsed.command else {
            panic!("revive command");
        };
        assert_eq!(value.name.as_deref(), Some("worker"));
        assert_eq!(value.expected_token.as_deref(), Some("generation"));
        assert!(value.dry_run);
        assert!(!value.all);
        assert_eq!(value.startup_timeout, 30.0);
        assert!(Cli::try_parse_from([
            "agentctl",
            "revive",
            "--all",
            "--dry-run",
            "--startup-timeout",
            "300"
        ])
        .is_ok());
        let help = Cli::try_parse_from(["agentctl", "revive", "--help"])
            .err()
            .unwrap()
            .to_string();
        for option in [
            "NAME",
            "--all",
            "--dry-run",
            "--expected-token",
            "--startup-timeout",
        ] {
            assert!(help.contains(option));
        }
    }

    #[test]
    fn revive_capability_requires_an_owned_interactive_adapter() {
        for (adapter, mode, expected) in [
            ("herdr", "interactive", true),
            ("herdr-pane", "interactive", true),
            ("herdr-relay", "interactive", true),
            ("herdr-foreign", "interactive", false),
            ("codex-app-server", "headless", false),
        ] {
            let mut record =
                json!({"name":"worker", "adapter":adapter, "mode":mode, "backend":"herdr"});
            add_capabilities(&mut record);
            assert_eq!(
                record["capabilities"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("revive")),
                expected,
                "{adapter}"
            );
        }
    }

    #[test]
    fn subcommands_have_local_help_and_registry_is_global() {
        Cli::command().debug_assert();
        for arguments in [
            ["agentctl", "--registry", "custom", "status", "worker"],
            ["agentctl", "status", "worker", "--state", "custom"],
        ] {
            assert_eq!(
                Cli::try_parse_from(arguments).unwrap().registry,
                PathBuf::from("custom")
            );
        }
        let error = Cli::try_parse_from(["agentctl", "send", "--help"])
            .err()
            .unwrap();
        let help = error.to_string();
        assert!(help.contains("--message-id"));
        assert!(help.contains("Seconds to wait"));
        assert!(!help.contains("--startup-timeout"));
        let error = Cli::try_parse_from(["agentctl", "adopt", "--help"])
            .err()
            .unwrap();
        let help = error.to_string();
        for required in ["--pane", "--workspace", "--cwd", "--harness", "--session"] {
            assert!(help.contains(required));
        }
        assert!(help.contains("without taking ownership"));
        assert!(help.contains("Muse requires a pinned foreground process"));
        let error = Cli::try_parse_from(["agentctl", "stop", "--help"])
            .err()
            .unwrap();
        let help = error.to_string();
        for required in [
            "--recover-legacy-adoption",
            "--retire-dead-adoption",
            "--expected-token",
            "--expected-record-sha256",
        ] {
            assert!(help.contains(required));
        }
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "status",
            "--bridge-state",
            "/tmp/chat-state",
            "--registry",
            "/tmp/registry",
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["agentctl", "chat", "quickstart"]).is_ok());
        let error = Cli::try_parse_from(["agentctl", "chat", "run", "--help"])
            .err()
            .expect("chat run help");
        let help = error.to_string();
        for required in [
            "--bridge-state",
            "--ready-timeout",
            "--working-timeout",
            "--reconcile-interval",
        ] {
            assert!(help.contains(required));
        }
        let error = Cli::try_parse_from(["agentctl", "chat", "publish", "--help"])
            .err()
            .expect("chat publish help");
        let help = error.to_string();
        for required in [
            "--bridge-state",
            "--channel-id",
            "--thread-id",
            "--request-id",
            "--file",
            "receipt.message_id",
        ] {
            assert!(help.contains(required));
        }
        let retry = Cli::try_parse_from([
            "agentctl",
            "chat",
            "retry-checkpoint-gap",
            "--bridge-state",
            "/tmp/chat-state",
            "--expected-gap-sha256",
            &"a".repeat(64),
            "--expected-checkpoint-sha256",
            &"b".repeat(64),
            "--expected-configuration-sha256",
            &"c".repeat(64),
            "--keep-cursor",
            "cursor-safe",
            "--evidence-file",
            "/tmp/evidence.json",
            "--evidence-sha256",
            &"d".repeat(64),
        ])
        .expect("parse explicit checkpoint retry");
        assert!(matches!(
            retry.command,
            Some(Commands::Chat(Chat {
                command: ChatCommand::RetryCheckpointGap(_),
            }))
        ));
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "retry-checkpoint-gap",
            "--bridge-state",
            "/tmp/chat-state",
        ])
        .is_err());
        let exact_retry = Cli::try_parse_from([
            "agentctl",
            "chat",
            "retry-boundary-gap",
            "--bridge-state",
            "/tmp/chat-state",
            "--expected-gap-sha256",
            &"a".repeat(64),
            "--expected-checkpoint-sha256",
            &"b".repeat(64),
            "--expected-configuration-sha256",
            &"c".repeat(64),
            "--keep-cursor",
            "cursor-safe",
            "--evidence-file",
            "/tmp/evidence.json",
            "--evidence-sha256",
            &"d".repeat(64),
        ])
        .expect("parse explicit exact-boundary retry");
        assert!(matches!(
            exact_retry.command,
            Some(Commands::Chat(Chat {
                command: ChatCommand::RetryBoundaryGap(_),
            }))
        ));
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "retry-boundary-gap",
            "--bridge-state",
            "/tmp/chat-state",
        ])
        .is_err());
        let help = Cli::try_parse_from(["agentctl", "chat", "retry-boundary-gap", "--help"])
            .err()
            .expect("exact-boundary help")
            .to_string();
        for option in [
            "--bridge-state",
            "--expected-gap-sha256",
            "--expected-checkpoint-sha256",
            "--expected-configuration-sha256",
            "--keep-cursor",
            "--evidence-file",
            "--evidence-sha256",
        ] {
            assert!(help.contains(option), "{option}");
        }
        assert!(help.contains("newer exact provider commit"));
        let accept = Cli::try_parse_from([
            "agentctl",
            "chat",
            "accept-checkpoint-boundary",
            "--bridge-state",
            "/tmp/chat-state",
            "--keep-cursor",
            "cursor-safe",
            "--expected-checkpoint-sha256",
            &"b".repeat(64),
            "--expected-configuration-sha256",
            &"c".repeat(64),
        ])
        .expect("parse checkpoint boundary acceptance");
        assert!(matches!(
            accept.command,
            Some(Commands::Chat(Chat {
                command: ChatCommand::AcceptCheckpointBoundary(_),
            }))
        ));
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "accept-checkpoint-boundary",
            "--bridge-state",
            "/tmp/chat-state",
            "--keep-cursor",
            "cursor-safe",
        ])
        .is_err());
        let help =
            Cli::try_parse_from(["agentctl", "chat", "accept-checkpoint-boundary", "--help"])
                .err()
                .expect("acceptance help")
                .to_string();
        for option in [
            "--bridge-state",
            "--keep-cursor",
            "--expected-checkpoint-sha256",
            "--expected-configuration-sha256",
        ] {
            assert!(help.contains(option), "{option}");
        }
        assert!(help.contains("every saved boundary message is a stored request"));
        let inspect = Cli::try_parse_from([
            "agentctl",
            "chat",
            "inspect",
            "--bridge-state",
            "/tmp/chat-state",
            "--request",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .expect("parse read-only request inspection");
        assert!(matches!(
            inspect.command,
            Some(Commands::Chat(Chat {
                command: ChatCommand::Inspect(ChatInspect { .. })
            }))
        ));
        let thread_last = |arguments: &[&str]| {
            let mut argv = vec![
                "agentctl",
                "chat",
                "thread",
                "--bridge-state",
                "/tmp/chat-state",
            ];
            argv.extend_from_slice(arguments);
            match Cli::try_parse_from(argv).map(|parsed| parsed.command) {
                Ok(Some(Commands::Chat(Chat {
                    command: ChatCommand::Thread(thread),
                }))) => Ok((thread.thread, thread.last)),
                Ok(_) => panic!("chat thread parsed as another command"),
                Err(error) => Err(error.to_string()),
            }
        };
        assert_eq!(
            thread_last(&["--thread", "spaces/example/threads/one"]),
            Ok(("spaces/example/threads/one".to_owned(), 10))
        );
        assert_eq!(
            thread_last(&["--thread", "-leading-hyphen", "--last", "100"]),
            Ok(("-leading-hyphen".to_owned(), 100))
        );
        assert!(thread_last(&["--thread", "t", "--last", "0"]).is_err());
        assert!(thread_last(&["--thread", "t", "--last", "101"]).is_err());
        assert!(thread_last(&["--last", "5"]).is_err());
        let error = Cli::try_parse_from(["agentctl", "chat", "thread", "--help"])
            .err()
            .expect("chat thread help");
        let help = error.to_string();
        for required in [
            "--bridge-state",
            "--thread",
            "--last",
            "(1-100)",
            "[default: 10]",
            "Example:",
        ] {
            assert!(help.contains(required), "{required} missing from:\n{help}");
        }
        let parsed = Cli::try_parse_from([
            "agentctl",
            "chat",
            "graceful-stop-main",
            "--main-pid",
            "123",
            "--main-pidfd-id",
            "456",
        ])
        .expect("parse internal systemd stop helper");
        assert!(matches!(
            parsed.command,
            Some(Commands::Chat(Chat {
                command: ChatCommand::GracefulStopMain(ChatGracefulStopMain {
                    main_pid: 123,
                    main_pidfd_id: 456,
                })
            }))
        ));
    }

    #[test]
    fn only_the_chat_service_and_its_stop_helper_timestamp_their_final_error() {
        let writes_service_log = |arguments: &[&str]| {
            Cli::try_parse_from(std::iter::once("agentctl").chain(arguments.iter().copied()))
                .expect("parse")
                .writes_service_log()
        };
        assert!(writes_service_log(&[
            "chat",
            "run",
            "--bridge-state",
            "/tmp/s"
        ]));
        assert!(writes_service_log(&[
            "chat",
            "graceful-stop-main",
            "--main-pid",
            "123",
            "--main-pidfd-id",
            "456",
        ]));
        assert!(!writes_service_log(&[
            "chat",
            "tick",
            "--bridge-state",
            "/tmp/s"
        ]));
        assert!(!writes_service_log(&[
            "chat",
            "status",
            "--bridge-state",
            "/tmp/s"
        ]));
        assert!(!writes_service_log(&["list"]));
        assert!(!writes_service_log(&[]));
    }

    #[test]
    fn chat_run_parses_repeated_runtime_only_ignored_text_prefixes() {
        let parsed = Cli::try_parse_from([
            "agentctl",
            "chat",
            "run",
            "--bridge-state",
            "/tmp/chat-state",
            "--ignore-text-prefix",
            "[assistant",
            "--ignore-text-prefix",
            "[notice]",
        ])
        .unwrap();
        let Some(Commands::Chat(Chat {
            command: ChatCommand::Run(run),
        })) = parsed.command
        else {
            panic!("expected chat run");
        };
        assert_eq!(run.ignored_text_prefixes, ["[assistant", "[notice]"]);
        for prefix in ["", "  ", "invalid\n"] {
            assert!(Cli::try_parse_from([
                "agentctl",
                "chat",
                "run",
                "--bridge-state",
                "/tmp/chat-state",
                "--ignore-text-prefix",
                prefix,
            ])
            .is_err());
        }
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "tick",
            "--bridge-state",
            "/tmp/chat-state",
            "--ignore-text-prefix",
            "[assistant",
        ])
        .is_err());
    }

    #[test]
    fn chat_reply_takes_a_reply_id_that_starts_with_a_hyphen_and_needs_every_argument() {
        let key = "0123456789abcdef".repeat(4);
        let arguments = [
            "--bridge-state",
            "/tmp/chat-state",
            "--request",
            key.as_str(),
            "--reply-id",
            "-AbC_1",
            "--file",
            "reply.md",
        ];
        let parsed =
            Cli::try_parse_from(["agentctl", "chat", "reply"].into_iter().chain(arguments))
                .unwrap();
        let Some(Commands::Chat(Chat {
            command: ChatCommand::Reply(reply),
        })) = parsed.command
        else {
            panic!("expected chat reply");
        };
        assert_eq!(
            reply.state.bridge_state,
            std::path::Path::new("/tmp/chat-state")
        );
        assert_eq!(reply.request, key);
        assert_eq!(reply.reply_id, "-AbC_1");
        assert_eq!(reply.file, std::path::Path::new("reply.md"));
        for missing in 0..4 {
            let rest = arguments
                .chunks(2)
                .enumerate()
                .filter(|(index, _)| *index != missing)
                .flat_map(|(_, pair)| pair.iter().copied());
            assert!(
                Cli::try_parse_from(["agentctl", "chat", "reply"].into_iter().chain(rest)).is_err(),
                "parsed without {}",
                arguments[2 * missing]
            );
        }
        // Only `chat run` takes the flag that offers the command, and it is off by default.
        for (flag, offered) in [(None, false), (Some("--offer-reply-command"), true)] {
            let parsed = Cli::try_parse_from(
                [
                    "agentctl",
                    "chat",
                    "run",
                    "--bridge-state",
                    "/tmp/chat-state",
                ]
                .into_iter()
                .chain(flag),
            )
            .unwrap();
            let Some(Commands::Chat(Chat {
                command: ChatCommand::Run(run),
            })) = parsed.command
            else {
                panic!("expected chat run");
            };
            assert_eq!(run.offer_reply_command, offered);
        }
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "tick",
            "--bridge-state",
            "/tmp/chat-state",
            "--offer-reply-command",
        ])
        .is_err());
    }

    #[test]
    fn chat_run_parses_the_opt_in_delivery_flags_and_both_default_off() {
        let run = |extra: &[&str]| {
            let parsed = Cli::try_parse_from(
                [
                    "agentctl",
                    "chat",
                    "run",
                    "--bridge-state",
                    "/tmp/chat-state",
                ]
                .iter()
                .chain(extra),
            )?;
            let Some(Commands::Chat(Chat {
                command: ChatCommand::Run(run),
            })) = parsed.command
            else {
                panic!("expected chat run");
            };
            Ok::<_, clap::Error>(run)
        };
        let defaults = run(&[]).unwrap();
        assert!(!defaults.deliver_while_working);
        assert_eq!(defaults.stale_composer_clear_after, None);
        let set = run(&[
            "--deliver-while-working",
            "--stale-composer-clear-after",
            "30m",
        ])
        .unwrap();
        assert!(set.deliver_while_working);
        assert_eq!(
            set.stale_composer_clear_after,
            Some(Duration::from_secs(1_800))
        );
        for bad in ["10s", "30", "1d", "soon"] {
            assert!(
                run(&["--stale-composer-clear-after", bad]).is_err(),
                "{bad}"
            );
        }
        for flag in [
            "--deliver-while-working",
            "--stale-composer-clear-after=30m",
        ] {
            assert!(Cli::try_parse_from([
                "agentctl",
                "chat",
                "tick",
                "--bridge-state",
                "/tmp/chat-state",
                flag,
            ])
            .is_err());
        }
    }

    #[test]
    fn chat_run_reminds_after_sixty_idle_seconds_unless_told_otherwise() {
        let run = |extra: &[&str]| {
            let parsed = Cli::try_parse_from(
                [
                    "agentctl",
                    "chat",
                    "run",
                    "--bridge-state",
                    "/tmp/chat-state",
                ]
                .iter()
                .chain(extra),
            )?;
            let Some(Commands::Chat(Chat {
                command: ChatCommand::Run(run),
            })) = parsed.command
            else {
                panic!("expected chat run");
            };
            Ok::<_, clap::Error>(run)
        };
        let defaults = run(&[]).unwrap();
        assert_eq!(
            defaults.reply_reminder_idle.0,
            Some(Duration::from_secs(60))
        );
        assert_eq!(defaults.claude_bin, std::path::Path::new("claude"));
        let set = run(&[
            "--reply-reminder-idle",
            "2m",
            "--claude-bin",
            "/opt/claude/bin/claude",
        ])
        .unwrap();
        assert_eq!(set.reply_reminder_idle.0, Some(Duration::from_secs(120)));
        assert_eq!(
            set.claude_bin,
            std::path::Path::new("/opt/claude/bin/claude")
        );
        assert_eq!(
            run(&["--reply-reminder-idle", "0"])
                .unwrap()
                .reply_reminder_idle
                .0,
            None,
            "0 turns reminders off"
        );
        for bad in ["5s", "13h", "25h", "1d", "soon"] {
            assert!(run(&["--reply-reminder-idle", bad]).is_err(), "{bad}");
        }
        for flag in ["--reply-reminder-idle=60s", "--claude-bin=claude"] {
            assert!(Cli::try_parse_from([
                "agentctl",
                "chat",
                "tick",
                "--bridge-state",
                "/tmp/chat-state",
                flag,
            ])
            .is_err());
        }
    }

    #[test]
    fn start_slot_options_parse_and_need_slot() {
        let parsed = Cli::try_parse_from([
            "agentctl",
            "start",
            "worker",
            "--slot",
            "s1",
            "--slot-isolation",
            "root",
            "--slot-project",
            "/p",
        ])
        .unwrap();
        let Some(Commands::Start(start)) = parsed.command else {
            panic!("expected start");
        };
        assert_eq!(start.slot.as_deref(), Some("s1"));
        assert_eq!(start.slot_isolation.as_deref(), Some("root"));
        assert_eq!(start.slot_project, Some(PathBuf::from("/p")));
        for arguments in [
            vec!["agentctl", "start", "worker", "--slot-isolation", "userns"],
            vec!["agentctl", "start", "worker", "--slot-project", "/p"],
            vec![
                "agentctl",
                "start",
                "worker",
                "--slot",
                "s1",
                "--slot-isolation",
                "namespace",
            ],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
        let parsed = Cli::try_parse_from([
            "agentctl",
            "start",
            "planner",
            "--project-box",
            "--box-writable",
            "project",
            "--box-isolation",
            "root",
            "--box-project",
            "/p",
            "--box-name",
            "lead",
        ])
        .unwrap();
        let Some(Commands::Start(start)) = parsed.command else {
            panic!("expected start");
        };
        assert!(start.project_box);
        assert_eq!(start.box_writable.as_deref(), Some("project"));
        assert_eq!(start.box_isolation.as_deref(), Some("root"));
        assert_eq!(start.box_project, Some(PathBuf::from("/p")));
        assert_eq!(start.box_name.as_deref(), Some("lead"));
        for arguments in [
            vec!["agentctl", "start", "w", "--box-writable", "project"],
            vec!["agentctl", "start", "w", "--box-isolation", "root"],
            vec!["agentctl", "start", "w", "--box-name", "lead"],
            vec!["agentctl", "start", "w", "--project-box", "--slot", "s1"],
            vec![
                "agentctl",
                "start",
                "w",
                "--project-box",
                "--box-writable",
                "all",
            ],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
        let help = Cli::try_parse_from(["agentctl", "start", "--help"])
            .err()
            .unwrap()
            .to_string();
        for flag in [
            "--slot <SLOT>",
            "--slot-isolation",
            "--slot-project",
            "--project-box",
            "--box-writable",
            "--box-isolation",
            "--box-project",
            "--box-name",
            "AGENTCTL_WRKSLOTS_BIN",
        ] {
            assert!(help.contains(flag), "{flag} missing from start help");
        }
    }

    #[test]
    fn stop_parser_preserves_all_legacy_recovery_assertions() {
        let parsed = Cli::try_parse_from([
            "agentctl",
            "stop",
            "legacy",
            "--recover-legacy-adoption",
            "--expected-token",
            "generation-1",
            "--expected-record-sha256",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ])
        .unwrap();
        let Some(Commands::Stop(stop)) = parsed.command else {
            panic!("expected stop command");
        };
        assert_eq!(stop.agent.name, "legacy");
        assert!(stop.recover_legacy_adoption);
        assert!(!stop.retire_dead_adoption);
        assert_eq!(stop.expected_token.as_deref(), Some("generation-1"));
        assert_eq!(
            stop.expected_record_sha256.as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn stop_parser_preserves_dead_adoption_assertions_and_refuses_both_selectors() {
        let digest = "a".repeat(64);
        let parsed = Cli::try_parse_from([
            "agentctl",
            "stop",
            "adopted",
            "--retire-dead-adoption",
            "--expected-token",
            "generation-1",
            "--expected-record-sha256",
            &digest,
        ])
        .unwrap();
        let Some(Commands::Stop(stop)) = parsed.command else {
            panic!("expected stop command");
        };
        assert!(stop.retire_dead_adoption);
        assert!(!stop.recover_legacy_adoption);
        assert_eq!(stop.expected_token.as_deref(), Some("generation-1"));
        assert_eq!(
            stop.expected_record_sha256.as_deref(),
            Some(digest.as_str())
        );
        let error = Cli::try_parse_from([
            "agentctl",
            "stop",
            "adopted",
            "--recover-legacy-adoption",
            "--retire-dead-adoption",
        ])
        .err()
        .unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn embedded_chat_guide_requires_derived_stop_and_measured_resource_bounds() {
        for required in [
            "hello_seconds + close_seconds + shutdown_grace_seconds + 7",
            "outbound_command.timeout_millis +",
            "maximum of the applicable provider window",
            "complete selected provider timeout tuple",
            "agentctl-chat-request-inspection/v1",
            "ack_completed_at_millis",
            "outcome-unknown operations",
            "systemd 258 or newer",
            "ExecStop=/absolute/path/agentctl chat graceful-stop-main",
            "TimeoutStopSec=<DERIVED_SECONDS_WITH_MARGIN>",
            "TimeoutStopFailureMode=kill",
            "TasksMax=<MEASURED_TASKS_WITH_HEADROOM>",
            "MemoryMax=<MEASURED_HARD_LIMIT>",
        ] {
            assert!(
                crate::CHAT_USER_GUIDE.contains(required),
                "embedded Chat guide omitted {required:?}"
            );
        }
        for stale in [
            "TimeoutStopSec=60",
            "TimeoutStopSec=75",
            "TimeoutStopSec=85",
            "TasksMax=2048",
            "MemoryMax=4G",
        ] {
            assert!(
                !crate::CHAT_USER_GUIDE.contains(stale),
                "embedded Chat guide retained stale prescription {stale:?}"
            );
        }
    }

    #[test]
    fn chat_publish_parses_exactly_one_bounded_input_shape_and_lowercase_uuid() {
        let uuid = "123e4567-e89b-42d3-a456-426614174000";
        let parsed = Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--request-id",
            uuid,
            "hello",
        ])
        .expect("positional publish text");
        let Some(Commands::Chat(Chat {
            command: ChatCommand::Publish(publish),
        })) = parsed.command
        else {
            panic!("expected publish command");
        };
        assert_eq!(publish.channel_id, "spaces/example");
        assert_eq!(publish.request_id, uuid);
        assert_eq!(publish.text.as_deref(), Some("hello"));
        assert!(publish.file.is_none());
        assert!(publish.thread_id.is_none());

        let threaded = Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--thread-id",
            "spaces/example/threads/one",
            "--request-id",
            uuid,
            "--file",
            "message.txt",
        ])
        .expect("threaded publish");
        let Some(Commands::Chat(Chat {
            command: ChatCommand::Publish(threaded),
        })) = threaded.command
        else {
            panic!("expected threaded publish command");
        };
        assert_eq!(
            threaded.thread_id.as_deref(),
            Some("spaces/example/threads/one")
        );

        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--request-id",
            uuid,
            "--file",
            "message.txt",
        ])
        .is_ok());
        for invalid in [
            "123E4567-e89b-42d3-a456-426614174000",
            "123e4567-e89b-12d3-a456-426614174000",
            "not-a-uuid",
        ] {
            assert!(Cli::try_parse_from([
                "agentctl",
                "chat",
                "publish",
                "--bridge-state",
                "/tmp/chat-state",
                "--channel-id",
                "spaces/example",
                "--request-id",
                invalid,
                "hello",
            ])
            .is_err());
        }
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--request-id",
            uuid,
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "agentctl",
            "chat",
            "publish",
            "--bridge-state",
            "/tmp/chat-state",
            "--channel-id",
            "spaces/example",
            "--request-id",
            uuid,
            "hello",
            "--file",
            "message.txt",
        ])
        .is_err());
    }
    #[test]
    fn invalid_timeouts_and_ambiguous_prompts_are_rejected() {
        for value in ["NaN", "inf", "-1", "31536001"] {
            assert!(seconds(value).is_err());
        }
        assert!(startup_seconds("0").is_err());
        assert!(positive_seconds("0").is_err());
        for option in ["--ready-timeout", "--working-timeout"] {
            assert!(Cli::try_parse_from([
                "agentctl",
                "chat",
                "run",
                "--bridge-state",
                "/tmp/chat-state",
                option,
                "30.000001",
            ])
            .is_err());
        }
        assert!(Cli::try_parse_from([
            "agentctl",
            "goal",
            "worker",
            "--goal-command-json",
            "[\"codex\",\"app-server\"]"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "agentctl", "send", "worker", "prompt", "--file", "task.txt"
        ])
        .is_err());
    }

    #[test]
    fn chat_capabilities_separate_code_installation_from_runtime_evidence() {
        let plugins = crate::plugins::PluginInventory {
            home: Some(PathBuf::from("/tmp/fixture-home")),
            discovered: vec![crate::plugins::DiscoveredPlugin {
                name: "fixture-chat".to_owned(),
                capability: "chat-subscription.fixture".to_owned(),
                executable: "backend".to_owned(),
                protocol_name: chat_subscription_plugin::PROTOCOL_NAME,
                protocol_min: 1,
                protocol_max: 1,
                origin: "discovered",
                status: crate::plugins::MaturityStatus::discovered(true),
                process_phase_timeouts:
                    chat_subscription_plugin::process::ProcessPhaseTimeouts::new(
                        std::time::Duration::from_secs(10),
                        std::time::Duration::from_secs(30),
                        std::time::Duration::from_secs(30),
                        std::time::Duration::from_secs(10),
                        std::time::Duration::from_secs(2),
                    )
                    .expect("fixture process phase timeouts"),
                executable_identity: None,
            }],
            refused: vec![crate::plugins::RefusedPlugin {
                entry: "old-chat".to_owned(),
                code: "incompatible_protocol",
                detail: "fixture refusal".to_owned(),
            }],
        };
        let document = capabilities_document(PathBuf::from("registry"), plugins, "herdr");
        assert_eq!(document["interactive"]["backends"], json!(["herdr"]));
        assert_eq!(
            document["term_backends"]["available"],
            json!(["herdr", "tmux"])
        );
        assert_eq!(document["term_backends"]["selected"], "herdr");
        assert_eq!(document["chat_subscriptions"]["core"]["origin"], "built_in");
        assert_eq!(
            document["chat_subscriptions"]["core"]["protocol_name"],
            "agentctl-chat-subscription"
        );
        let discovered = &document["chat_subscriptions"]["plugins"]["discovered"][0];
        assert_eq!(discovered["origin"], "discovered");
        assert_eq!(discovered["status"]["implemented"], true);
        assert_eq!(discovered["status"]["configured"], false);
        assert_eq!(discovered["status"]["connected"], false);
        assert_eq!(discovered["status"]["live_verified"], false);
        assert_eq!(
            document["chat_subscriptions"]["plugins"]["refused"][0]["code"],
            "incompatible_protocol"
        );
        assert_eq!(
            document["chat_subscriptions"]["reference_backends"][0]["status"]["implemented"],
            false
        );
        assert_eq!(
            document["chat_subscriptions"]["reference_backends"][0]["available"],
            json!(["design"])
        );
        assert!(
            document["chat_subscriptions"]["reference_backends"][0]["missing"]
                .as_array()
                .expect("missing capability list")
                .contains(&json!("rust-provider"))
        );
    }

    #[test]
    fn agentcloud_start_flags_parse_document_and_refuse_misuse() {
        let parsed = Cli::try_parse_from([
            "agentctl",
            "--agentcloudctl-bin",
            "/opt/bin/agentcloudctl",
            "start",
            "sub-cloud",
            "--harness",
            "agentcloud",
            "--cloud-harness",
            "claude-code",
            "--provision",
            "--envspec",
            "monorepo",
            "--purpose",
            "sub-cloud checkout",
            "--agentterm-bin",
            "/opt/bin/agentterm",
        ])
        .expect("agentcloud start parses");
        assert_eq!(
            parsed.agentcloudctl_bin,
            PathBuf::from("/opt/bin/agentcloudctl")
        );
        assert_eq!(parsed.agentterm_bin, PathBuf::from("/opt/bin/agentterm"));
        let Some(Commands::Start(start)) = parsed.command else {
            panic!("expected start");
        };
        assert!(start.provision);
        assert_eq!(start.cloud_harness.as_deref(), Some("claude-code"));
        for invalid in [
            vec![
                "start",
                "sub-cloud",
                "--harness",
                "agentcloud",
                "--envspec",
                "monorepo",
            ],
            vec![
                "start",
                "sub-cloud",
                "--harness",
                "agentcloud",
                "--purpose",
                "label",
            ],
            vec![
                "start",
                "sub-cloud",
                "--harness",
                "agentcloud",
                "--provision",
                "--node-id",
                "n1",
            ],
            vec![
                "start",
                "sub-cloud",
                "--harness",
                "agentcloud",
                "--cloud-harness",
                "claude",
            ],
        ] {
            assert!(
                Cli::try_parse_from(std::iter::once("agentctl").chain(invalid.iter().copied()))
                    .is_err(),
                "accepted {invalid:?}"
            );
        }
        let help = Cli::try_parse_from(["agentctl", "start", "--help"])
            .err()
            .expect("start help")
            .to_string();
        for required in [
            "--cloud-harness",
            "--provision",
            "--envspec",
            "--purpose",
            "--cloud-workspace",
            "--node-id",
            "--agentcloudctl-bin",
            "--agentterm-bin",
            "agentterm -s SESSION_ID",
            "300",
        ] {
            assert!(help.contains(required), "start help omitted {required:?}");
        }
        for (command, required) in [
            ("stop", "--skip-cloud-halt"),
            ("send", "idempotency"),
            ("read", "--output last"),
            ("wait", "--until settle"),
        ] {
            let help = Cli::try_parse_from(["agentctl", command, "-h"])
                .err()
                .expect("short help")
                .to_string();
            assert!(help.contains(required), "{command} -h omitted {required:?}");
        }
    }

    #[test]
    fn agentcloud_flags_are_usage_errors_outside_agentcloud_launches() {
        let registry = std::env::temp_dir().join(format!(
            "agentctl-cli-cloud-{}-{}",
            std::process::id(),
            line!()
        ));
        let registry = registry.display().to_string();
        for arguments in [
            vec![
                "--registry",
                registry.as_str(),
                "--herdr-bin",
                "/nonexistent/herdr",
                "start",
                "sub-local",
                "--harness",
                "codex",
                "--provision",
            ],
            vec![
                "--registry",
                registry.as_str(),
                "--herdr-bin",
                "/nonexistent/herdr",
                "start",
                "sub-local",
                "--profile",
                "cloud",
                "--cloud-workspace",
                "/w",
            ],
        ] {
            let code = main(arguments.iter().map(OsString::from));
            assert_eq!(code, 2, "{arguments:?}");
        }
        assert!(!Path::new(&registry).exists());
        assert_eq!(
            main(["skill", "install", "--agentterm-bin", "/x"].map(OsString::from)),
            2
        );
    }

    #[test]
    fn an_empty_from_session_pins_a_human_send_even_inside_a_session() {
        let inside = Some("c0ffee00-0000-4000-8000-000000000001".to_owned());
        assert_eq!(caller_session(Some(String::new()), inside.clone()), None);
        assert_eq!(caller_session(None, inside.clone()), inside);
        assert_eq!(
            caller_session(Some("a".to_owned()), inside),
            Some("a".to_owned())
        );
        assert_eq!(caller_session(None, Some(String::new())), None);
        assert_eq!(caller_session(None, None), None);
    }

    #[test]
    fn agentcloud_records_advertise_only_the_cloud_interface() {
        let mut record = json!({
            "name": "sub-cloud",
            "adapter": "agentcloud",
            "mode": "interactive",
            "backend": "herdr",
        });
        add_capabilities(&mut record);
        assert_eq!(record["capabilities"], json!(CLOUD_CAPABILITIES));
        assert!(!record["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("drain")));
    }

    #[test]
    fn adopted_interactive_records_advertise_the_full_named_interface() {
        let mut record = json!({
            "name": "foreign",
            "adapter": "herdr-foreign",
            "mode": "interactive",
            "backend": "herdr",
        });
        add_capabilities(&mut record);
        assert_eq!(
            record["capabilities"],
            json!([
                "send",
                "status",
                "read",
                "wait",
                "stop",
                "attach",
                "pause",
                "resume",
                "terminal-snapshot",
                "drain",
                "goal",
                "bind-session",
                "anchor",
                "rename"
            ])
        );
    }
}
#[test]
fn ordinary_session_control_does_not_discover_plugins() {
    assert!(plugin_discovery_required(&Commands::Capabilities));
    assert!(!plugin_discovery_required(&Commands::List));
    assert!(!plugin_discovery_required(&Commands::Status(Named {
        name: "worker".to_owned(),
    })));
}
