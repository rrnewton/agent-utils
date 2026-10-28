# agent-utils: related work for the suite

Checked 2026-10-18. This document does two things. It indexes the per-tool comparisons, each of
which compares one tool with the public systems closest to it. It then compares the suite as a
whole with other systems that orchestrate a team of coding agents, along the axis that matters most
when choosing one: **what runs where**.

Claims resting on source code cite the file at a pinned revision. Claims resting on vendor
documentation link the page. Claims that could not be confirmed for this review are marked
**unverified**, and no conclusion here rests on one.

## Per-tool comparisons

| Tool | Comparison | What it is compared with |
| --- | --- | --- |
| `agentctl` and `agentctl chat` | [common/docs/agentctl/RELATED_WORK.md](common/docs/agentctl/RELATED_WORK.md) | Remote session control (Happy), chat channels into a harness (Claude Code's Discord and Telegram plugins), structured session APIs (Codex app-server), terminal dashboards (Claude Squad), coordinator inboxes (Gas Town, CLI Agent Orchestrator), and restoring a team after a restart (tmux-resurrect, systemd, Monit). |
| `wrkslots` | [common/docs/wrkslots/RELATED_WORK.md](common/docs/wrkslots/RELATED_WORK.md) | Git and standalone worktree managers (Worktrunk, WTP, git-worktree-runner), agent harnesses that create worktrees (Claude Code, Codex, OpenCode, Vibe Kanban, Gas Town, Parallel Code), jj workspaces, ephemeral CI runners, and Kubernetes controllers. |
| `herdr-run` | [common/docs/herdr-run/RELATED_WORK.md](common/docs/herdr-run/RELATED_WORK.md) | Herdr's own command API, tmux, OpenSSH, and Pexpect. This is also where Herdr, the suite's terminal server, is compared with tmux. |
| `wrkviz` | [common/docs/wrkviz/RELATED_WORK.md](common/docs/wrkviz/RELATED_WORK.md) | Transcript browsers and trace viewers, above all AgentsView, plus Perfetto and AgentPrism. |
| `gh-paced` | [common/docs/gh-paced/RELATED_WORK.md](common/docs/gh-paced/RELATED_WORK.md) | Client-side GitHub rate limiting: GitHub CLI's cache, octokit's throttling plugin, Bottleneck, PyGithub, go-github. |
| `agent-usage` | [common/docs/agent-usage/RELATED_WORK.md](common/docs/agent-usage/RELATED_WORK.md) | The harnesses' own `/status` screens, ccusage, Claude Code Usage Monitor, and status-line scripts. |
| `vibe-talk` | [vibe-talk/RELATED_WORK.md](vibe-talk/RELATED_WORK.md) | Happy Coder, Claude mobile voice mode with a Discord connector, ElevenLabs Agents with a phone number, Discord bots, and voice frameworks. |
| `dagrun` | [common/docs/dagrun/PLANNER_DESIGN.md, section 3](common/docs/dagrun/PLANNER_DESIGN.md#3-related-work) | Scheduling literature (speedup models, moldable-task allocation) only. There is no comparison with other CI runners or build schedulers yet. |
| `minibeads` (submodule) | [ai_docs/transient/2026-10-28-task-tracker-adapters/README.md, section 6](ai_docs/transient/2026-10-28-task-tracker-adapters/README.md#6-trackers-surveyed) | A working comparison, not yet a settled document: upstream beads, GitHub Issues through `gh`, and Gas Town's task layer, by storage, states, dependencies, claims and closing. minibeads' own repository describes it as a lighter replacement for upstream beads. |

No comparison exists yet for `tick-hub`, `pr-landing-planner`, `parallel-experiment-runner`, or
`cpuset-alloc` (a companion command of `dagrun`). The documentation policy in
[AGENTS.md](AGENTS.md#documentation) asks for one per top-level tool, so these are open gaps, not
judgements that nothing comparable exists. The [website](site/index.html) states the same gap.

## The suite as a whole: what runs where

### Why this axis

A team of coding agents has four parts that can each live in a different place: the
**orchestrator** that decides what each agent does and keeps the team's state, the **agents**
themselves (their processes, checkouts, and tools), the **model inference** each agent calls, and
the **front end** a person uses to watch and steer. Each part can run on one of three kinds of
machine:

- a **headless development server** the developer controls, reached over the network;
- the **laptop** the developer is sitting at;
- a **vendor's servers**, operated by someone else under that vendor's account and terms.

Systems that look alike in a feature list differ sharply here, and the difference decides who can
see the code, what stops working when a laptop lid closes, and which account a team's state
belongs to.

### Where agent-utils stands

agent-utils is local-only for a developer: the orchestrator, every agent, and all of the team's
state run on one host the developer controls, typically a headless development server. No
vendor-operated service schedules the agents or holds the team's state. In detail:

- **The orchestrator is an ordinary coding agent in a terminal on that host.** Setup makes the agent
  started in the harness directory the coordinator, running as its native TUI in a tab of
  [Herdr](https://github.com/herdrdev/herdr), a persistent terminal server
  ([README.md](README.md#the-suite)). The session registry and launch profiles are private files in
  the harness, `.agentctl/` ([agent-utils-setup](skills/agent-utils-setup/SKILL.md)).
- **Agents run beside it, each in its own checkout.** `agentctl` starts each subagent as its native
  harness TUI in another Herdr tab; `wrkslots` gives each a Git worktree slot with systemd resource
  limits and, optionally, a file-system box where only the slot is writable
  ([README.md](README.md#the-suite)).
- **Supporting work runs on the same host too.** Validation is a local dependency graph run by
  `dagrun`, and hosted CI is "a signal for a human, not a gate" ([AGENTS.md](AGENTS.md)). Recurring
  duties come from a local timer running `tick-hub` and reach the coordinator through
  `agentctl send`. `herdr-run` executes allowlisted commands, such as `git push`, in a visible
  pane on the host. `gh-paced` shares its GitHub budget between processes through a locked file on
  each host rather than a server
  ([gh-paced related work](common/docs/gh-paced/RELATED_WORK.md)). `agent-usage` reads plan usage
  without a model call ([README.md](README.md#the-suite)).
- **The laptop is a window, not a host.** A person attaches to any agent's Herdr tab and can take
  over (`agentctl attach`). Closing the laptop stops nothing. How the laptop reaches the server's
  terminals (SSH, for example) is left to the developer; the suite does not supply or require a
  relay.
- **The phone front end is a server the owner runs.** `vibe-talk` is "a bridge, not an agent
  host": it holds the chat credential and serves a phone web app, and it is deliberately not
  co-located with the agents. It "starts in Podman on a laptop and is expected to move to a small
  cloud host unchanged", one the owner operates. The project would "rather own that front door
  than hand it to a vendor" ([vibe-talk/README.md](vibe-talk/README.md)).

**Where vendors still appear.** "No vendor in the loop" is true of orchestration and team state.
It is not true of every byte, and the suite does not claim otherwise:

- **Model inference.** The suite drives existing harnesses (Claude Code, Codex, and others) and
  does not host or proxy their models. Those harnesses call their vendors' model APIs, so prompts
  and the code context in them leave the host.
- **The project's remote.** Branches are published to the project's Git host, GitHub for this
  repository.
- **Chat.** The `agentctl chat` bridge reaches the owner through a chat provider; Google Chat
  works today through a polling transport ([README.md](README.md#the-suite)). `vibe-talk` reads
  and posts through Discord, or through a compatible HTTP bridge to other services
  ([vibe-talk/README.md](vibe-talk/README.md)).
- **Voice, optionally.** `vibe-talk` uses ElevenLabs for voice conversations by default; its
  Read control can use the phone's own speech engine instead, with no ElevenLabs account
  ([vibe-talk/README.md](vibe-talk/README.md)).
- **Remote workers, opt-in.** `agentctl` has an adapter (`agentcloud` in `agentctl capabilities`)
  for workers on separately provisioned remote nodes, for a task that needs its own machine
  ([agentctl skill](skills/agentctl/SKILL.md)). Nothing in a default harness uses it.

Each of these is an endpoint the owner holds a credential for and can replace; none of them owns
the team's state or decides what an agent does next.

### Comparison

| Part | agent-utils | Gas Town | Hermes Agent (all **unverified**) |
| --- | --- | --- | --- |
| Orchestrator | A coding-agent TUI (the coordinator) in a Herdr tab on the developer's host, with file-based registries in the harness. | The `gt` command and a coordinating agent session in tmux on the developer's host (tmux use verified from source; the "Mayor" role name **unverified**). | The `hermes serve` backend, which owns the agent; reported to run locally or on a remote server the user operates. |
| Agents | Native harness TUIs (Claude Code, Codex, and others) in Herdr tabs on the same host, one wrkslots worktree each. | "Polecats": agent sessions in tmux, each in a Git worktree sandbox on the same host (verified from source). | Executes on self-hosted infrastructure, local or remote. Whether it runs a team of separate agent processes was not established. |
| Model inference | Whatever each harness uses; for Claude Code and Codex, the vendor's API. | Whatever the driven harness uses; it drives Claude Code through hooks (verified from source). | Reported to work with local or remote inference. |
| Front end | Herdr attach from any terminal; chat through `agentctl chat`; phone and voice through the owner's own `vibe-talk` server. | tmux sessions a person can attach to (tmux verified from source). Other front ends not reviewed. | A desktop thin client that connects to a local or remote `hermes serve`. |
| Team state and memory | Files and Git on the host: the agentctl and wrkslots registries, slot branches, transcripts. | Files and Git on the host: a "town" directory and Beads issue state ([agent-log-archive](scripts/agent-log-archive/README.md) archives both). | Reported persistent memory, and skills as Markdown documents. |
| What a closed laptop stops | Nothing; the laptop only attaches. | Nothing, if the town runs on a server; everything, if it runs on the laptop. | Depends on where `hermes serve` runs: the thin client is only a viewer. |
| Vendor-operated control plane | None. | None found. | None reported. |

### Gas Town

[Gas Town](https://github.com/steveyegge/gastown) is the closest system in shape: a local
coordinator directing many coding agents, each in its own Git worktree and tmux session, with
recovery handled by recurring reconciliation. The Gas Town facts in this document come from source read at commit
`649b832b` for the [wrkslots comparison](common/docs/wrkslots/RELATED_WORK.md#gastown) on
2026-08-29 and from a module snapshot at the same commit for the
[agentctl comparison](common/docs/agentctl/RELATED_WORK.md#coordinator-inbox-telling-a-supervisor-that-workers-need-it)
on 2026-09-29. They were not re-read for this document.

- Worktrees are polecat sandboxes whose reclamation is a fail-closed verdict over many facts:
  agent state, hook work, Git dirtiness, unpushed commits, merge-queue state, and a live tmux check
  ([workstate.go](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/internal/polecat/workstate.go),
  [reclaim.go](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/internal/polecat/reclaim.go)).
- A recurring patrol retries interrupted cleanup
  ([lifecycle patrol design](https://github.com/steveyegge/gastown/blob/649b832b7672bc7a2dbef26f5983aba6198b819b/docs/design/polecat-lifecycle-patrol.md)).
- `gt nudge` waits for an idle tmux pane, then falls back to an on-disk queue that a Claude hook
  drains at the next turn boundary.

On the axis of this document, Gas Town and agent-utils agree: both put the orchestrator, the
agents, and the team's state on a machine the developer controls, and the Gas Town source
reviewed shows no vendor control plane.
They differ in structure rather than location. Gas Town is one integrated product with its own
roles and its own issue tracker; agent-utils is a set of independently installable tools, any of
which can be used alone, around unmodified harness TUIs in a terminal server built for agents
rather than tmux. The [wrkslots comparison](common/docs/wrkslots/RELATED_WORK.md#gastown) records
the narrower differences in worktree recovery.

### Hermes Agent

[Hermes Agent](https://hermes-agent.nousresearch.com/docs) is an open-source agent from Nous
Research. Its documentation could not be fetched for this review: the forward proxy refused the
documentation site (HTTP 403), and the sandbox this review ran in could not run `herdr-run`, the
suite's route around such refusals. Every statement about it here is therefore **unverified**,
taken from secondary descriptions rather than the vendor's own pages or source:

- It is open source and self-hosted: the user runs it on their own machine or a server they
  operate.
- A backend, started with `hermes serve`, owns the agent; a desktop application is a thin client
  that connects to that backend, local or remote.
- It can use a local model or a remote inference provider.
- It keeps persistent memory across sessions and stores skills as Markdown documents.

If those descriptions are accurate, Hermes sits close to agent-utils on the axis that matters: the
user, not a vendor, runs the part that owns the agent, and a client device is only a viewer. It
differs in kind, because it is an agent rather than a harness for running other vendors' agents.
Its unit is one agent with its own memory and skills; agent-utils' unit is a team of existing
coding agents in isolated checkouts, and its skills are instructions for those agents rather than
memory they accumulate. The only Hermes-specific advantage on this axis is local inference, which
agent-utils does not offer because it adopts each harness's own model access. Before relying on any
of this, check it against the [documentation](https://hermes-agent.nousresearch.com/docs).

### Other systems on the same axis

The per-tool comparisons cover two more systems that answer the question differently:

- [Happy](https://github.com/slopus/happy) controls coding-agent sessions from a phone or the web
  through a synchronization server and Happy account authentication, hosted or self-hosted. That
  server sits in the control path between the developer and the agents
  ([agentctl comparison](common/docs/agentctl/RELATED_WORK.md#happy-the-closest-remote-control-comparison)).
- [Claude Squad](https://github.com/smtg-ai/claude-squad) runs agents in tmux and Git worktrees on
  the developer's own machine, with no synchronization server in the path
  ([agentctl comparison](common/docs/agentctl/RELATED_WORK.md#claude-squad-visible-workers-and-isolated-workspaces)).

## Bottom line

Running the orchestrator and agents on the developer's own machine is not new: Gas Town and Claude
Squad already do it, as the cited source shows. What agent-utils adds on this axis is the
combination of three properties: everything that owns the team's state runs on one host the
developer controls; the agents are unmodified harness TUIs that a person can attach to and take
over at any moment; and each remaining vendor dependency (model inference, the Git host, the chat
provider, optional voice) is a separately replaceable endpoint rather than a control plane. That
conclusion rests on this repository's own documents and the pinned Gas Town, Happy, and Claude
Squad sources. It does not rest on the unverified Hermes descriptions.
