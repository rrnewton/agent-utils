# suite-a-la-carte: does each agent-utils tool work on its own?

Working design and audit, 2026-10-24. `#498 suite-a-la-carte`. Code references are to `main` at
`2a9d8012` unless a line says otherwise. The team-messaging design it refers to is
[`2026-10-24-agentctl-team-messaging.md`](2026-10-24-agentctl-team-messaging.md)
(`#496 agentctl-team-messaging`). A separate lane is adding a section there on where team
messaging lives; section 2.5 below is this audit's input to that decision.

## 0. Summary

The README promises that "every tool also stands alone and has an independently installable
distribution". The audit mostly confirms it. No Python distribution imports another, except
`parallel-experiment-runner`, which declares its dependency on `dagrun`. The Rust crates depend
only on the chat-subscription libraries. Tools are composed through process interfaces (a
command named by an environment variable, a project-owned liveness command) and through harness
templates, not through each other's code. The standalone check added with this document now
proves two subsets, `wrkslots` and `tick-hub`, by running their quickstart workflows with every
other suite command absent.

Three real problems, and some friction:

1. **agentctl is four products in one crate.** The Rust `agentctl` crate holds agent management
   (the registry, queue, drainer, Herdr adapter, harness TUIs and agentcloud), the chat bridge
   host (about 46% of the crate's Rust lines), the coordinator inbox and the agent watcher.
   Team messaging is designed to join them. The coupling between chat and agent management is
   real and mostly one way, but it is wider than one interface: chat also uses the composer
   layer, the target lock, credentials and inbox helpers, and the core carries three chat-born
   pieces of its own (section 2.2).
2. **The Rust chat bridge has no provider that a public user can run.** The repository's
   Slack provider (an unpublished crate built separately) reaches Slack only through a separately installed Slack command-line client; its Web
   API transport is a placeholder that refuses every call. The public Google Chat transport
   exists only in the Python edition. So "chat bridging only", on the Rust edition, means
   writing a plugin (section 3.1).
3. **Herdr is a hard dependency of every agentctl feature that types into an agent**, including
   chat and the planned team messaging. That is a design choice, not an accident: delivery
   means typing into a terminal pane that agentctl drives, and the single-typist lock lives on
   that pane. But it means that "team messaging without
   subagents" still means "agentctl and Herdr" (section 3.2).
4. **Friction, not hard dependencies:** `wrkslots init` defaults new projects to disk-image
   slots, so the first `create` on a machine without passwordless `sudo` or `fuse2fs` refuses
   (with a remedy). wrkslots requires a liveness command that the project writes. The guided
   setup skill only builds the full coordinator harness. The README said that no chat provider
   ships for the Rust bridge; this change corrects it (sections 3.1, 3.3, 3.8).

**Recommendation for agentctl:** keep one `agentctl` command. Split the crate into an
`agentctl-core` library (registry, queue, drainer, target lock, Herdr adapter, composer layer,
harness TUIs, subagents, credentials, inbox), an `agentctl-chat` library (the bridge host), and
later an `agentctl-teams` library. This is a refactor of weeks, not days: section 2.2 lists what
must first become a public core API and what must move out of core. The binary enables chat and teams through default-on Cargo features.
Add a CI build of the core with neither feature, plus a dependency check that core never
depends on chat and teams never depend on chat. Version the on-disk registry and queue format
explicitly, because that format, not the binary, is the contract between the CLI and the
long-running services that pin older builds. A separately shipped bridge binary stays a cheap
later step (section 2.4).

**Periodic check:** `scripts/check_standalone.py` (section 4) installs a subset into a bare
virtual environment and gives it a fresh `HOME` and a `PATH` with every suite command, Herdr,
and the system commands the subset claims not to need removed.
It proves that no other suite command or sibling package is reachable, then runs the subset's
quickstart and a scenario. It runs as `packages.standalone.tick-hub` and
`packages.standalone.wrkslots` in `validation.dag.yaml`. Section 4.3 proposes the next subsets,
in order of value.

Owner questions are in section 6.

## 1. Inventory

### 1.1 Per tool

"Suite calls" lists every place a tool runs, imports or reads another suite tool. "Hard" means
the tool cannot do its main job without it. "Optional" means a flag or configuration selects it.
"Glue" means the connection lives in the harness, not in the tool.

| Tool | Distributions | Install needs | Run-time needs outside the suite | Suite calls |
|---|---|---|---|---|
| `agentctl` | Python `agentctl` (also `herdr-agent`, `herdr-subagents`, `herdr-chat`); Rust crate `agentctl` (also `herdr-agent`) | Python 3.10, no dependencies (`py/agentctl/pyproject.toml`). Rust: crates `chat-subscription`, `chat-subscription-plugin` (`rs/agentctl/Cargo.toml:15-16`) | Herdr for interactive agents and both chat bridges; the harness CLIs (Codex, Claude Code, Muse); tmux as an alternative transcript view for Python headless workers; systemd user units only for `install-units` and `restore --boot`; `agentcloudctl` and `agentterm` only for the agentcloud adapter (`rs/agentctl/src/cli.rs:2156-2159`) | **Optional:** `wrkslots`, only for `start --slot` or `--project-box`, found through `AGENTCTL_WRKSLOTS_BIN`, then `"wrkslots"` in `.agentctl/profiles.json`, then `PATH` (`rs/agentctl/src/subagents.rs:3952-4013`, `py/agentctl/subagents.py:6038-6057`). No dependency on `herdr-run`; both have their own Herdr adapters (README "Persistent coding agents") |
| `agentctl chat` (Rust) | Part of the `agentctl` crate (`chat_runtime.rs`, `chat_service.rs`, `chat_events.rs`, `stale_composer.rs`) | As above | A provider plugin in `$AGENTCTL_HOME/plugins/<name>/` speaking `agentctl-chat-subscription` v1, and an outbound helper; a target agent registered in an agentctl registry and live in a Herdr pane (`embedded_chat_quickstart.md` step 1) | **Hard:** agentctl's own queue, drainer and pane capture (section 2.2) |
| Slack provider | Rust crate `chat-subscription-slack` (`publish = false`): `agentctl-slack-plugin`, `agentctl-slack-outbound` | Crates `chat-subscription`, `chat-subscription-plugin`; `agentctl` only as a dev-dependency for tests (`rs/chat-subscription-slack/Cargo.toml:13-20`) | A Slack command-line client at an operator-configured absolute path. The Web API transport refuses: "not implemented in this release; configure the cli transport" (`rs/chat-subscription-slack/src/transport.rs:438-456`) | Runs as a child of `agentctl chat run` |
| `agentctl chat` (Python) | Part of the Python `agentctl` distribution (`chat.py`, `chat_*.py`) | As above | Google Chat's public REST API with an operator-provided OAuth token command (`py/agentctl/chat.py:1472-1484`), or an operator transport command or socket; Herdr | **Hard:** agentctl's queue and drainer |
| `agentctl inbox`, `agentctl watch-agents` | Part of the Rust crate | As above | Herdr (the watcher reads panes); `agentcloud-notify` delivery needs agentcloud | Read agentctl's registry |
| `wrkslots` | Python `wrkslots`; Rust `wrkslotsd` is an unpublished read-only replay of its event log | Python 3.10, no dependencies | Git and Linux `/proc`. Disk-image slots: `mkfs.ext4` and passwordless `sudo` or `fuse2fs` with `/dev/fuse`. `run`, `box` and `shell-command`: a systemd user manager (`systemd-run`, `busctl`) and user namespaces or `sudo` (`common/docs/wrkslots/README.md`, "Install") | **Glue:** the required `liveness_command` (`py/wrkslots/cli.py`, `REQUIRED_CONFIG_KEYS`) is project-owned. The package ships a generic fixture probe and an agentctl probe (`py/wrkslots/examples/`); this harness uses the agentctl one. Agent names must follow agentctl's naming rule: `create`, `adopt` and `transfer-owner` refuse others (`py/wrkslots/cli.py:594-600`) |
| `dagrun`, `cpuset-alloc` | Python `dagrun` (both commands); Rust crate `dagrun` (both binaries) | Python: PyYAML. Rust: ordinary crates | cgroup v2 through a systemd user manager, required by default; `--allow-cgroup-failure` accepts running without it (`py/dagrun/README.md`) | **Optional:** `gh-paced` for GitHub calls (below) |
| `parallel-experiment-runner` | Python only | `dagrun>=0.13.1` (`py/parallel_experiment_runner/pyproject.toml`) | systemd user manager (through dagrun) | **Hard, declared:** `dagrun` |
| `herdr-run` | Python `herdr-run`; Rust crate `herdr-run` | None (its own YAML-subset parser in Rust) | Herdr at a fixed path and a systemd user manager (`py/herdr_run/README.md:92-95`) | None |
| `tick-hub` | Python `tick-hub`; Rust crate `tick-hub` | Python: PyYAML | None | **Glue:** the setup skill's `tick-deliver` template pipes ticks to `agentctl send` (`skills/agent-utils-setup/SKILL.md:431-440`). tick-hub itself only prints |
| `pr-landing-planner` | Python and Rust | Python: PyYAML | Git; `gh` | **Optional:** `gh-paced` |
| `wrkviz` | Python only | None | Harness transcripts; a harness CLI for model summaries; `gh` for GitHub enrichment | **Optional:** `gh-paced` |
| `gh-paced` | Rust only, unpublished (`publish = false`) | Cargo | The real `gh` | None |
| `agent-usage` | Rust only, unpublished | Cargo | Claude Code and Codex logins; Codex's app-server | None |
| `vibe-talk` | A service with its own Cargo workspace, container image and CI (README "Services") | Cargo or Podman | A chat provider: Discord, Slack's Web API, or a compatible HTTP bridge (none ships here); ElevenLabs optional (`vibe-talk/README.md`) | None. Nothing in `vibe-talk/src` names agentctl |
| Per-tool skills | `skills/<tool>/` | none | the tool | Each names only its own tool, except that the wrkslots skill mentions agentctl twice as an example caller |
| Suite skills | `agent-utils-setup`, `agent-utils-coordinator` | none | Herdr | Assume the whole harness: "Core tools, always set up: agentctl ... wrkslots" (`skills/agent-utils-setup/SKILL.md:49`) |

How GitHub access is shared deserves a note, because it is the pattern the rest of the suite
should copy. `dagrun`, `pr-landing-planner` and `wrkviz` each carry an exact copy of
`common/gh_resolve/gh_resolve.py`, kept identical by `py/tests/test_gh_resolve_copies.py`,
"because every tool is published as an independent distribution that may not import a sibling".
The resolver prefers `AGENT_UTILS_GH`, then `gh-paced`, then plain `gh`
(`py/dagrun/gh_resolve.py:1-25, 120-160`). The tools gain pacing when it is installed and lose
nothing when it is not.

### 1.2 The dependency graph

```text
                       Herdr (external) ◄───────────────┬───────────────┐
                          ▲  hard                        │ hard          │ hard
                          │                              │               │
  chat-subscription ◄── agentctl (Rust crate) ── optional ──► wrkslots   herdr-run
  chat-subscription-plugin ▲   │  agent mgmt · chat host · inbox · watcher    ▲
        ▲                  │   │                                              │ glue: liveness
        │ crate            │   └──── process protocol ──► provider plugins    │ command reads
  chat-subscription-slack ─┘          ($AGENTCTL_HOME/plugins/*)              │ agentctl's registry
  (dev-dependency only)                    └── Slack CLI client (external)    │
                                                                       harness bin/agent-liveness
  tick-hub ── glue (tick-deliver → agentctl send) ──► agentctl

  dagrun ◄── hard (declared) ── parallel-experiment-runner
  dagrun, pr-landing-planner, wrkviz ── optional (vendored resolver) ──► gh-paced ──► gh

  vibe-talk, agent-usage, wrkslotsd: no suite edges
```

The only hard edges between suite tools are `parallel-experiment-runner → dagrun`, which is
declared, and chat's dependence on agentctl's queue, which lives inside one crate. Every other
edge is optional or lives in harness glue. Existing checks enforce the import side:
`scripts/check_python_packages.py` installs each wheel alone and asserts that no sibling package
can be imported (`:1010-1024`), and `scripts/check_deps.py` runs every entry point with PyYAML
blocked. Nothing checked the run-time side (a tool shelling out to another suite command, or
needing Herdr) until the standalone check in section 4.

## 2. agentctl's factoring

### 2.1 What is in the binary

The Rust `agentctl` binary has 35 top-level subcommands (the `Commands` enum,
`rs/agentctl/src/cli.rs:67-202`). They fall into four groups:

| Group | Subcommands | Rust source lines, tests included (embedded guides excluded) |
|---|---|---|
| Agent management | `capabilities start profiles skill adopt move rename anchor promote restore install-units watch doctor stop revive list status send drain prompts answer read wait goal bind-session codex-goal attach attach-self pause resume` | `agent.rs`, `client.rs`, `submission.rs`, `subagents.rs` and `subagents/`, `harness_tui/`, `profiles.rs`, `restore.rs`, `units.rs`, `tui_prompts.rs`, `codex_goal*`, `skill_install.rs`, `credentials.rs`, `cli.rs`, and the chat-born `plugins.rs` (1,756) and `prompt_time.rs` (633): about 65,300 (the remainder) |
| Chat bridge host | `chat` (20 subcommands: `init status grant grants revoke grant-limit ... run reply close`) | `chat_runtime.rs` and `chat_runtime/`, `chat_service.rs` and `chat_service/`, `chat_events.rs`, `stale_composer.rs`: about 60,000 |
| Supervision | `inbox`, `watch-agents` | `inbox.rs` and `inbox/`, `agent_watch.rs`: about 6,400 |
| Planned: team messaging | `teams ...`, `team ...` (#496 section 11) | not written |

Chat providers are already outside the crate. `chat-subscription` (2,900 lines) and
`chat-subscription-plugin` (9,900) are libraries, and the Slack provider (9,200) is its own crate
with its own binaries, reached over a versioned process protocol. So the provider layer is
already a plugin architecture. The open question is only about the bridge host.

### 2.2 Where the coupling is real

**Chat depends on agent management, through more than one interface.** The chat modules import
`agent::{DrainOptions, QueueMessageState}` and `subagents::{ManagedAgents, ManagedApi}`
(`chat_runtime.rs:34-35`), plus `AgentRuntime`, `HerdrClient` and `StaleComposer`
(`chat_service.rs:29-40`). Outside tests, chat uses:

- prompt delivery through the internal trait `CoordinatorDelivery`: `submit`, `drain`,
  `message_state` and their reporting variants (`chat_runtime.rs:4544-4600`). The
  implementation enqueues into the target agent's durable queue and runs agentctl's own drainer
  (`chat_service.rs:2936-2970`);
- pane observation: `pane_info`, `read_capture`, `peek_capture` (with and without a runtime),
  and `pause`, which is all `chat_service.rs` calls on the manager;
- Herdr's event socket, for wakeups on pane output (`client.event_socket`,
  `client.event_socket_with_cancellation`);
- the harness composer layer: `submission::composer_view`, `composer_is_empty`, `running_turn`
  and `clear_composer` (`stale_composer.rs:112-113, 214, 355`; `running_turn` also in
  `chat_service.rs`). The bridge clears a stale composer itself, so it types into the pane
  directly as well as through the queue;
- the host-wide target lock, `agent::lock_target_within` (`stale_composer.rs:540`), the same
  lock the drainer takes;
- credential inspection, `credentials::{problems, inspect, failure_explanation,
  CREDENTIAL_EXPIRY_WARNING}` (`chat_runtime.rs:6837-6838` and nearby, `chat_service.rs`);
- inbox helpers for reply reminders, `inbox::{environment_value, claude_sessions_by_pane,
  transcript_of_session}` (`chat_service/reply_reminders.rs:205, 329, 374`);
- many private-file helpers from `agent` (`open_private_lock`, `atomic_json`, `sync_directory`,
  `validate_private_directory`, and others).

The first two items are the essential coupling. The bridge types owner messages into the same
queue that `agentctl send` uses, and the drainer takes the host-wide target lock before it types,
so a chat message and a CLI message can never be pasted into one composer at once. The bridge
reads replies from the same pane that `agentctl read` reads. Any factoring must keep exactly one
implementation of the queue, the drainer and the lock. Herdr is required for a separate reason:
delivery means typing into a terminal pane, and Herdr is the pane host agentctl drives. The other
items are ordinary shared code that a core library can export.

**Agent management depends on chat in three places.** Outside `cli.rs`:

- `prompt_time.rs` is a chat feature: "The time stamp that opens every chat request prompt". Core
  carries it because a queued prompt stores its deferred opening and create time (`sent_at`,
  `opening`), written by `agent::enqueue` (`agent.rs:838-845`) and retimed by the drainer
  (`agent.rs:2927-2935`). Those fields are part of the queue format, so this piece belongs in
  core with the queue, not in chat.
- The Herdr client runs every `herdr` command through
  `chat_subscription_plugin::process::ProcessPluginChild` (`client.rs:6, 1099`), a process
  supervision primitive from the chat plugin library. A core-only build would still link
  `chat-subscription-plugin` unless that primitive moves to a neutral crate.
- `plugins.rs` (chat-provider plugin discovery) is reached from the core command
  `capabilities` (`cli.rs:1367, 2148-2150`). `capabilities` needs a way to report chat
  plugins only when the chat feature is built.

There are two small leftovers: `prompt_time.rs:100` borrows `chat_runtime::unix_millis()`, and
the chat helper `subagents::chat_capture_source` (`subagents.rs:73-79`) lives in the wrong
module.

**The shared state is the real contract.** The bridge, the agent watcher and the CLI all read
and write the same registry and queue directories. In practice they already run different
builds: a deployment that runs the bridge and the watcher as long-lived services naturally pins
a copy of the binary for each, while the coordinator runs whatever CLI is current. The team-messaging
design names "which binary drives that registry" among the facts an operator has to know by hand
and that go stale (#496 section 0). Splitting binaries would not create version skew. The skew exists today; what
is missing is an explicit, versioned format that tells a reader its writer was newer.

**Supervision** (`inbox`, `watch-agents`) reads the registry and panes and posts notices. It
depends on the core in the same way chat does, and on nothing in chat. Chat depends on it,
through the inbox helpers above.

### 2.3 Options

| | A. One binary, separable crates (recommended) | B. Separate binaries sharing crates | C. Runtime plugins for the host | D. Status quo |
|---|---|---|---|---|
| Shape | `agentctl-core`, `agentctl-chat`, later `agentctl-teams` libraries in the workspace; the `agentctl` binary enables `chat` and `teams` through default-on Cargo features | `agentctl` (core, perhaps with teams) and `agentctl-chat` (the bridge) as separate binaries, both linking core | `agentctl` runs `agentctl-chat` like a Git external subcommand, or loads it dynamically | One crate, one binary |
| Single typist | Preserved: one queue library | Preserved, if both link the same core library | External subcommand: preserved (it links core). A queue IPC API instead of linking: a second correctness boundary to prove | Preserved |
| Packaging | Unchanged for users. `cargo install agentctl --no-default-features` gives a lean core | Two artifacts. A chat user installs both, because adopt and status live in core | As B, plus discovery rules | Unchanged |
| Versioning | One version | Two versions that must agree on the on-disk format, which needs explicit versioning either way | As B | One version, but pinned services skew anyway |
| Pinned deployments | Unchanged: services keep pinning a copy of `agentctl` | Simplifies them: the bridge unit pins `agentctl-chat` and the CLI upgrades freely | As B | Unchanged |
| Cost to build | Weeks: the `pub(crate)` items chat uses (section 2.2) become a public core API, `ProcessPluginChild` moves to a neutral crate, `cli.rs` dispatch and `capabilities` move behind features | A, plus a second binary target and packaging checks | A, plus a dispatcher. A dynamic library is ruled out: Rust has no stable ABI | None |
| What it buys | Enforced layering: core cannot grow chat dependencies. Team messaging lands on core without touching chat. A core-only build for people who never bridge chat | A's benefits, plus independent upgrades of the long-running bridge | Like B, discovered at run time | Nothing |

**Python edition.** The modules are already grouped (`chat*.py`, `mcp.py`, `foreign/`), and the
distribution has no third-party dependencies, so extras would save nothing at install. Leave it
as one distribution. Whether the Python chat bridge keeps getting features is a separate
question (section 6, question 2).

### 2.4 Recommendation

Choose **A**, and treat B as a later packaging decision rather than an architecture change:

1. Extract `agentctl-core`: registry, queue (including `prompt_time`'s deferred openings, which
   are part of its format), drainer, target lock, Herdr client, composer layer (`submission`),
   harness TUIs, subagents, profiles, restore, credentials, the private-file helpers, and
   `inbox` and `watch-agents` (chat uses the inbox helpers, and they are small). Make the
   `CoordinatorDelivery`, pane-observation, composer and lock functions chat uses the core's
   public API. Move `unix_millis` and `chat_capture_source`.
2. Move `ProcessPluginChild` (and whatever it needs) from `chat-subscription-plugin` into a
   small neutral crate that both the Herdr client and the plugin protocol depend on, so core
   stops linking the chat plugin library.
3. Move the bridge host and `plugins.rs` into `agentctl-chat`, which depends on core,
   `chat-subscription` and `chat-subscription-plugin`. `capabilities` asks the chat feature for
   its plugin inventory when it is built.
4. Build team messaging as `agentctl-teams` on core. Its "bridges beside a contact" checks
   (#496 section 9) read bridge state files; they must not link chat code, and they must treat
   "no bridge configured" as normal.
5. In the `agentctl` binary, gate `chat` and `team`/`teams` behind default-on features. `agentctl
   capabilities` already reports services and plugins; it should report which features this
   build has, so `team` in a build without it fails with exit 2 as #496 section 11 already
   specifies for an edition that lacks it.
6. Enforce the layering in validation: build `-p agentctl --no-default-features`, and fail if
   `cargo tree -p agentctl-core` or `-p agentctl-teams` reaches `agentctl-chat` or
   `chat-subscription-plugin`.
7. Give the registry and queue a format version that readers check, with a clear refusal when a
   writer was newer. This is the contract between pinned services and the CLI under any option.

If the owner later wants the bridge to be its own artifact (B), add an `agentctl-chat` binary
target to the chat crate and let `agentctl chat` run it Git-style when the feature is compiled
out. Neither step changes the libraries.

## 3. Standalone-subset scenarios

Each scenario lists what a user installs and configures, what works, and the gaps. The gaps
marked **accidental** are ones a change could remove without changing the design.

### 3.1 Only a chat bridge to an existing agent session (no subagents)

- **Install:** Herdr; the Rust `agentctl` crate; a provider plugin and outbound helper. The
  agent must already run in a Herdr pane.
- **Configure:** register the running session with `agentctl adopt NAME --pane ... --workspace
  ...` (four identity assertions, `embedded_quickstart.md:90-101`) or `agentctl attach-self`;
  write an owner-only `chat.json`; `agentctl chat init`; run `agentctl chat run` under a
  supervisor of the user's choice.
- **Works without:** `start`, profiles, wrkslots, every other suite tool.
- **Gaps:**
  - **Accidental:** no public provider for the Rust bridge. The Slack plugin's only working
    transport runs a separately installed Slack command-line client whose command shape is
    fixed (`slack.message list`, `slack.user describe`, `rs/chat-subscription-slack/README.md`).
    The Web API transport refuses. A public user with only a Slack bot token cannot bridge. The
    fix is to implement `WebApiTransport` against `conversations.history`, `chat.postMessage`
    and `reactions.add`, or to ship a Google Chat plugin for the Rust host.
  - **Accidental, fixed here:** the README's suite table said the Rust bridge's providers "plug
    in through the subscription-plugin protocol, for which no provider ships yet"
    (`README.md:59`). The Slack provider has shipped since then; this change replaces that
    sentence with the narrower truth above.
  - **By design:** Herdr. The bridge types into the pane and reads replies from it. The Claude
    Code Discord and Telegram plugins show the alternative: messages enter one harness as MCP
    notifications, with no supervisor or multiplexer
    ([agentctl comparison](../../common/docs/agentctl/RELATED_WORK.md#claude-code-channels-chat-connects-to-the-harness)).
    That path is harness-specific and would be a different product, not a factoring.
  - Python edition: Google Chat works through its public API, and Herdr is still required.

### 3.2 Only team messaging between coordinators (no subagents)

Per the #496 design, which is not implemented yet.

- **Install:** `agentctl` on every host that runs a coordinator, and Herdr there (agentcloud
  contacts aside). A way to run a command on the other host: local, `ssh`, or an exec template.
- **Configure:** `agentctl adopt` (or `attach-self`) for the coordinator; `agentctl team init
  TEAM`; an owner-level `teams.yaml`.
- **Works without:** `start`, profiles, wrkslots, chat. Team messaging needs the registry, the
  queue lanes and tombstones (#496 sections 7.1-7.4), the drainer and process-ancestry identity:
  all core.
- **Gaps to avoid while building it:** do not make `team accept` require a chat bridge; the
  bridge checks must be advisory where none is configured. Keep the envelope versioned
  (`--protocol 1`) so hosts on different builds interoperate or refuse cleanly
  (`remote-too-old`). The "fat binary" concern is answered by section 2.4: a team-messaging
  user gets core plus teams, and need not build chat at all.

### 3.3 Only wrkslots (proven by the check)

- **Install:** `python3 -m pip install wrkslots`. Git.
- **Configure:** `wrkslots init . --worktrees-dir worktrees/slots --liveness-command PATH` with a
  project-written liveness command.
- **Proven:** with no agentctl or Herdr, and with `sudo`, `systemd-run`, `systemctl`, `busctl`,
  `mount` and the image tools withheld from `PATH`, an agent creates its own slot,
  commits, writes a handoff and releases. The coordinator reads the handoff and removes the slot,
  and the agent's branch keeps its commit (`scripts/check_standalone.py`, `wrkslots_scenario`).
- **Accidental friction:**
  - `init` writes `slot_representation: image` for every new project
    (`py/wrkslots/cli.py:4151-4154`) without checking that an image backend exists. On a machine
    without passwordless `sudo` or `fuse2fs`, the first `create` refuses and leaves a create
    journal that must be recovered. The refusal names the remedy (`create --representation
    worktree`). The README's install section lists Git and `/proc` as the requirements and the
    image tools as "additionally" needed, which reads as optional, yet they are needed by
    default. Either `init` should choose `worktree` when no backend is usable, or it should say
    so when it writes `image`.
  - `liveness_command` is required, and the shipped generic example answers from fixture files.
    A user without agentctl has to write one. A small `pid-file` probe (the agent writes its PID;
    dead means that process is gone) would make the no-agentctl path complete.
  - The default `sandbox.home_shared` list names harness and Herdr directories. It is harmless
    when they do not exist.

### 3.4 Only dagrun

- **Install:** `pip install dagrun` (PyYAML) or `cargo install dagrun`.
- **Run:** cgroup v2 through a systemd user manager by default; `--allow-cgroup-failure` where
  delegation is unavailable. No suite tool; `gh-paced` is used only if present.
- **Not yet in the check:** running dagrun inside a dagrun-run validation step needs either
  `--allow-cgroup-failure` or the graph's `delegated_children` mechanism. Proposed in 4.3.

### 3.5 Only vibe-talk

- **Install:** its container image, or a native build of its own Cargo workspace.
- **Configure:** a chat provider credential (Discord, or Slack through its Web API; Google Chat
  needs a compatible HTTP bridge, which this repository does not ship); ElevenLabs optionally,
  or the phone's speech engine for reading.
- **No suite tool** is involved; it is a bridge to chat, not to agentctl. Its own CI covers its
  smoke test (`vibe-talk.smoke.cli-help`).

### 3.6 Only tick-hub (proven by the check)

`pip install tick-hub`, write `ops.yaml`, run `tick-hub tick` from any timer. The check runs the
README quick start: a due reminder fires, `--flush` persists state, and the next tick emits
nothing. Delivering ticks to an agent is the harness's job (`tick-deliver`), so a user can pipe
them anywhere.

### 3.7 The others, briefly

| Subset | Install and configure | Gaps |
|---|---|---|
| `herdr-run` | Herdr, a systemd user manager, `.herdr-run.yaml` | None; Herdr is the point of the tool |
| `agentctl` for subagents, without wrkslots | Herdr, harness CLIs, `agentctl start` | None: slots are optional |
| `gh-paced`, `agent-usage` | `cargo build` from this repository; neither is published | Not on any package index, so "standalone" means a source checkout |
| `wrkviz` | `pip install wrkviz`; transcripts in the harness's usual places | Model summaries need a harness CLI; the timeline works without |
| `pr-landing-planner` | `pip install pr-landing-planner`, `gh` | None |
| `parallel-experiment-runner` | `pip install parallel-experiment-runner` brings `dagrun` | None: declared |

### 3.8 The guided setup is all-or-nothing

`agent-utils-setup` always builds the coordinator harness with agentctl, wrkslots and Herdr,
and offers the rest as add-ons (`skills/agent-utils-setup/SKILL.md:49-52`). That is right for
its purpose. But someone who wants only team messaging, or only wrkslots, gets no guided path:
the per-tool READMEs and quickstarts are the subset path today. A "single tool" mode in the
setup skill (install one tool and its skill, without building a harness) would close this.

### 3.9 Accidental hard dependencies, collected

| Where | What | Severity | Fix |
|---|---|---|---|
| Rust chat bridge | Only provider needs a non-public Slack CLI client | Blocks public use of the Rust bridge | Implement the Slack Web API transport, or ship a public provider |
| `wrkslots init` | Defaults to disk images without checking for a backend | First `create` refuses; remedy printed | Choose `worktree` when no backend is usable, or say so at init |
| `wrkslots` | Liveness command must be written by the project | Friction | Ship a PID-file probe |
| agentctl crate | Chat helpers in core modules; core pieces `pub(crate)` | Blocks a core-only build | Section 2.4 |
| README | Said no Rust chat provider ships | Misled | Corrected in this change |

No tool was found that needs another suite tool's binary or configuration just to start.

## 4. A periodic standalone check

### 4.1 What existed

| Check | Proves | Does not prove |
|---|---|---|
| `scripts/check_deps.py` | Every Python entry point's `--help`, `--version` and usage error work with PyYAML blocked | Anything past argument parsing |
| `scripts/check_python_packages.py` | Each wheel and sdist builds, installs alone, imports no sibling, answers `--help`, `--version`, its user guide and (agentctl) `quickstart` | Run-time behaviour. Commands inherit the validator's `PATH`, so a tool that shells out to another suite command would still pass |
| `scripts/check_rust_packages.py` | Each published crate packages and builds in isolation | Run-time behaviour |
| `vibe-talk/tests.dag.yaml` | vibe-talk's own suite and CLI smoke | n/a: already separate |

### 4.2 What this change adds

`scripts/check_standalone.py` keeps a table of subsets. For each one it:

1. builds each of the subset's `py/` projects as a wheel from a temporary copy (`pip wheel
   --no-deps --no-index --no-build-isolation`; the source tree gains no build output) and
   installs it into a fresh virtual environment created without pip and without the
   interpreter's own site-packages, so its console scripts start that environment's
   interpreter and see nothing installed on the host;
2. copies each declared third-party dependency's installed files (PyYAML for tick-hub) from the
   validating interpreter into that environment, and fails if a declared dependency names no
   files to copy;
3. builds a directory of links to every ordinary system command from the standard system
   directories, except every command the suite installs, Herdr, and the system commands the
   subset says it does not need (for wrkslots: `sudo`, `systemd-run`, `systemctl`, `busctl`,
   `fuse2fs`, `mkfs.ext4`, `mount`, `umount`). The suite's command and package lists are read
   from the tree (`py/bin/*`, every `[[bin]]` in the Cargo manifests, every `py/*` project), so
   a new tool is excluded automatically;
4. runs every command with an empty environment: a fresh `HOME`, `TMPDIR` and XDG directories,
   `PATH` set to the environment's `bin` plus that directory, `PYTHONNOUSERSITE=1`. Each command
   leads its own process group, and a timeout kills the whole group;
5. proves isolation before the scenario: the subset's own commands resolve into its environment
   and start its interpreter, no other suite command or withheld system command resolves, and
   no other suite package imports from inside the environment;
6. runs `COMMAND quickstart`, then the subset's scenario.

A scenario that tries a foreign command fails with "not installed in this subset's
environment". Five negative probes were run by hand against the tick-hub subset: dropping its
PyYAML declaration, calling `agentctl`, importing `dagrun`, calling a withheld `sudo`, and a
command that outlives its timeout with a background child. All five failed as they should, and
the timed-out process group left nothing running. An earlier version ran commands through the
validating interpreter with `PYTHONPATH`; the adversarial review showed that this left the
interpreter's own site-packages visible, which is why the environment is now a bare virtual
environment.

Wiring: two steps in `validation/packages.dag.yaml`, group `standalone`, labelled
`validation-packages`, `validation-python-packages` and the tool's component label. A change to
wrkslots or tick-hub source, to Python packaging, or to the checker itself (a new `PREFIX_RULES`
entry and `validate.py --self-test` expectation) selects them. A full `validate-all` always runs
them. Cost measured on the development host, a busy shared workstation:

| Step | Wall | CPU | Where the time goes |
|---|---|---|---|
| `packages.standalone.tick-hub` | 1.6-1.7 s | about 1.7 s | wheel build, install and three ticks |
| `packages.standalone.wrkslots` | 42 s | about 18 s user, 23 s system | almost all in `wrkslots remove`, whose process census scales with the host's process count. A runner with few processes should take a few seconds |

The wrkslots step declares `cpu_timeout: 300` and `timeout: 900` for that reason.

### 4.3 Next subsets, in order of value

| Subset | Scenario | What it needs | Where |
|---|---|---|---|
| agentctl core | `adopt` a fake agent, `send`, `drain`, `status`, `read` with no wrkslots or chat | The existing fake Herdr (`py/tests/herdr_fake.py`, `cross/herdr_e2e/fake_herdr.py`) placed on the subset's `PATH` as `herdr`: the one listed exception to "no Herdr" | `packages.standalone`, Python edition first |
| agentctl chat, Rust | `chat init` and `chat tick` against `chat-subscription-fake-plugin` and the fake Herdr | A built `agentctl` and fake plugin; reuse the `rust-packages` build so the step adds no compile | `rust-packages` fragment, after `repository.build.rust-launchers` |
| agentctl core without chat | `cargo build -p agentctl --no-default-features`, plus a `cargo tree` layering check | Section 2.4 | `rust` fragment |
| team messaging | Two registries on one host, `team init`, `team send` over `local` reach, `team receipt`, with no chat configured | #496 phase 3 | `packages.standalone` |
| dagrun | Run a two-step example with `--allow-cgroup-failure`, then `list` and `viz` | Nothing new | `packages.standalone` |
| herdr-run | `init`, then a refused and an admitted command through the fake Herdr | The fake Herdr | `packages.standalone` |
| Rust editions | `cargo install --path rs/<crate> --offline --root TMP`, then the same scenarios | One build per crate (minutes); nightly only | a nightly `validate-all` label |
| wrkslots without agentctl, image slots | The same lifecycle with `--representation image` where `sudo -n` works | A privileged runner | nightly, skipped elsewhere with the reason printed |

The check is a table, so each row above is a scenario function and a `Subset` entry. Commands
that the scenario genuinely needs (the fake Herdr) go in the subset's `commands` and are placed
on its `PATH` explicitly. The check then documents each dependency it allows.

## 5. Composability compared with Gas Town and other systems

Sources: Gas Town read at commit `c0b37bd5` (cloned 2026-10-24) for this section; the other
systems as recorded, with pinned revisions, in [RELATED_WORK.md](../../RELATED_WORK.md),
[the agentctl comparison](../../common/docs/agentctl/RELATED_WORK.md) and
[the wrkslots comparison](../../common/docs/wrkslots/RELATED_WORK.md).

| System | Unit of installation | Smallest useful piece, and what it needs | Messaging between agents |
|---|---|---|---|
| agent-utils | One distribution per tool (eight Python distributions; seven published Rust crates, five with commands and two libraries; unpublished `gh-paced`, `agent-usage` and the Slack provider) and a separately deployed service (`vibe-talk`) | Any one tool; for example wrkslots needs Python and Git (section 3.3), tick-hub needs Python and PyYAML | `agentctl send` into a durable per-agent queue; needs agentctl and Herdr. Team messaging designed on the same queue |
| Gas Town | One `gt` command (`cmd/gt`), plus `gt-proxy-client` and `gt-proxy-server`, with its roles (Mayor, crew, polecats, Witness, Refinery, Deacon) built in (`README.md`, "Core Concepts") | A town: `gt install` creates a headquarters directory. Native installs list Beads (`bd`) 0.57 or newer, sqlite3, and tmux for the tmux-backed roles as prerequisites, and the Linux and Windows steps install Dolt first; a "minimal mode" runs agents by hand without tmux or the daemon, but still through `gt` and Beads (`README.md`, "Prerequisites"; `docs/INSTALLING.md`, "Minimal Mode vs Full Stack Mode") | `gt mail` stores messages as beads through the `bd` command (`internal/mail/bd.go`); `gt nudge` waits for an idle tmux pane, then falls back to an on-disk queue that a Claude Code hook drains (RELATED_WORK.md, at commit `649b832b`). Federation between towns goes through DoltHub (`README.md`, "Wasteland") |
| Claude Squad | One TUI program | The whole program; tmux and Git worktrees | Not covered by the comparison; a person attaches to and reprompts each worker |
| CLI Agent Orchestrator | One Python package | The orchestrator; its inbox keeps messages in a database until the receiver is idle | Built in |
| Happy | A wrapper CLI, clients, and a synchronization server | Remote control needs the server (hosted or self-hosted) and a Happy account | Through the server |
| Claude Code channel plugins | One MCP server per chat service | A chat channel into one Claude Code session, no supervisor or multiplexer | Not agent-to-agent |
| Worktrunk, WTP, git-worktree-runner | One worktree manager each | Worktree creation and cleanup, alone | None |

What the comparison shows:

- **Gas Town is integrated by design.** Its parts share one binary and one state store (Beads on
  Dolt), and its README describes them as roles in one town. A user who wants only its worktree
  sandboxes, or only its mail, installs `gt`, `bd` and Dolt and creates a town. That buys
  consistency, with one tracker and one model of work, at the price of adopting all of it.
- **agent-utils puts the boundaries between tools.** Its tools share no store. wrkslots learns
  whether an agent lives from a command the project names; tick-hub prints and a template
  delivers; GitHub pacing is a resolver each tool vendors. Section 3 shows that this holds in
  practice for every tool except the agentctl features that type into an agent, which share
  agentctl's queue and Herdr by design.
- **The narrowest pieces elsewhere are single-purpose tools.** The worktree managers and the
  Claude Code channel plugins each do one thing. wrkslots already sits in the first group, and
  the check now proves it. A chat bridge that needs neither Herdr nor a registry exists only in
  the harness-specific MCP form; agentctl's bridge trades that for working with any harness
  that runs in a terminal.
- **Messaging is where every system couples.** Gas Town's mail needs Beads, CLI Agent
  Orchestrator's inbox needs its database, and agentctl's queue needs agentctl and Herdr. The
  à la carte question for team messaging is therefore not whether it can avoid a store, but
  whether its store can be installed without the rest. Section 2.4's core-plus-teams build is
  this suite's answer.

## 6. Questions for the owner

1. **agentctl factoring.** Approve option A: core, chat and teams as separate crates behind
   default-on features of one `agentctl` binary, with a CI build of core alone and the rule that
   teams never depend on chat? Or go straight to a separate bridge binary (B), since the
   deployment already pins the bridge separately?
2. **A public chat provider for the Rust bridge.** Implement the Slack Web API transport (or a
   Google Chat plugin for the Rust host), so the Rust bridge works for a public user? Or is the
   Python edition's Google Chat bridge the public story, with Rust for the owner's deployment?
3. **wrkslots default.** Keep `image` as the default for new projects, which dogfoods isolation
   but fails first on machines without `sudo -n` or `fuse2fs`, or have `init` choose `worktree`
   when no image backend is usable?
4. **A single-tool setup path.** Should `agent-utils-setup` gain a mode that installs one tool
   and its skill without building a coordinator harness?
5. **The fake Herdr in the standalone check.** For the next subsets (agentctl core, chat,
   herdr-run), is it acceptable to put the test suite's fake Herdr on the subset's `PATH`? The
   alternative is a nightly runner with a real Herdr.
