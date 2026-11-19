# Harness support matrix

What `agentctl` actually does for each coding-agent harness, from the code.

agentctl drives harnesses it does not control through their terminal
screens, their command lines, and the files they write. Almost every such
fact is specific to one harness: the glyph in front of its composer, the
key that submits a staged prompt, where it writes its transcript, whether
a follow-up typed mid-turn steers, queues, or is refused. Those facts live
behind one adapter per harness (#574 harness-adapters), and the support
levels diverge — in places nobody intended. The owner asked for one table
that says, honestly, what each harness gets (owner 082, 2026-11-15;
#1561 harness-support-matrix).

This document is that table. It records the code's truth on the day it was
written, not what any guide or skill claims. Where a guide or skill claims
more than the code does, the claim is listed under
[Claims the code does not back](#claims-the-code-does-not-back) at the end.

## The rule

**Any change that adds or removes support for one harness updates this
matrix in the same commit.** A new per-harness branch, a new adapter
method, a harness added to or dropped from a list, a fix that works on
one harness first — each changes a cell here, and the commit that changes
the code changes the cell. The same rule is stated in this repository's
`AGENTS.md` and in the agentctl skill; a change that lands without the
matrix update is incomplete.

## How to read this

| Mark | Meaning |
| --- | --- |
| supported | the capability works for this harness today |
| partial | it works with a named limitation |
| none | agentctl does not do this for this harness |
| in progress | a named lane is changing this cell now |

Columns:

- **Claude Code**, **Codex**, **Muse**: the interactive TUIs, driven in a
  terminal pane (Herdr or tmux) by the Rust edition, which is the edition
  the tracked launchers run. Per-cell differences in the Python edition
  are called out in [The Python edition](#the-python-edition).
- **Unknown TUI**: any other TUI, driven by harness-neutral screen rules
  (`--harness unknown`, #850 unknown-harness-mode). It knows no phrase,
  glyph, launch argument, or transcript of the program it drives.
- **Headless**: a harness driven over its structured protocol instead of
  a terminal (#565 headless-coordinator). Today this build runs only
  Claude Code headless; the cell says so wherever it matters.

Every cell names its code path. Paths are relative to this repository;
`rs/` is the Rust edition, `py/` the Python edition.

## The matrix

| Capability | Claude Code | Codex | Muse | Unknown TUI | Headless |
| --- | --- | --- | --- | --- | --- |
| Send: typing + submit | supported | supported | supported | partial | supported (Claude only) |
| Second Enter after delivery (#1691) | supported | supported | supported (delayed re-check, #1732) | none | n/a (no composer) |
| Send: delivery receipts | supported | supported | supported | partial | supported (Claude only) |
| Follow-up while working | partial | supported | supported | none | n/a (queued turns) |
| Stuck-paste / Enter backstop | supported | partial | supported | partial | n/a (no composer) |
| Composer / draft detection | supported | supported | supported | none | n/a |
| Busy / idle detection | supported | supported | supported | partial | supported (event log) |
| Interrupt a running turn | supported | supported | supported | partial | supported (Claude only) |
| Approvals relay to chat | none | none | none | none | supported (Claude only) |
| Forms / questions (`prompts`, `answer`) | supported | supported | none (detect only) | none | n/a |
| Trust dialogs | supported | supported | partial | none | n/a |
| Headless mode itself | supported | none | in progress | none | — |
| Transcript reading | supported | supported | supported | none | supported (typed events) |
| `--trace-thread` harvesting | supported | supported | supported | none | supported (typed events) |
| `tail` / `console` / `read` | partial | partial | partial | partial | supported |
| Session identity | supported | supported | supported | none | supported (Claude only) |
| Takeover / handback | n/a (is the target) | none | none | none | supported (Claude only) |
| Restore / revive (launch replay) | supported | supported | partial | none | supported (Claude only) |
| Effort control | supported | supported | supported | none | supported (Claude only) |
| Model selection | supported | supported | supported | none | supported (Claude only) |
| Launch-env replay | supported | supported | supported | partial | supported |
| Permission bypass at launch | supported | supported | supported | n/a | supported |
| Chat-bridge delivery | supported | supported | supported | partial | supported (Claude only) |
| Watcher notices (DONE / BLOCKED / idle / stalled) | supported | supported | supported | partial | none (skipped) |
| Raw messages (no sender header) | supported | supported | supported | partial | supported (Claude only) |
| Goal commands | supported | supported | supported | none | partial |
| Skill installation | supported | supported | supported | none | n/a |
| Boxed launch (`--slot`, userns) | none | none | supported | none | n/a |
| Transient-failure recovery | supported | supported | none | none | none |
| Stalled-goal resume | none | supported | none | none | none |

The rows below give, for each capability, the one-line truth per harness,
the issue that tracks it, and the code that implements it. A reader who
only reads the table above gets the shape; a reader changing one cell
reads its row first.

## Row notes

### Send: typing + submit

Verified submission — stage the text in an empty composer, press the
submission key, prove the text left the composer — is implemented per
adapter (`HarnessAdapter::verified_submission`, `composer_view`,
`submit_key` in `rs/agentctl/src/harness/mod.rs`; driver in
`rs/agentctl/src/submission.rs`).

- **Claude Code**: supported. Framed and unframed composer readers
  (`rs/agentctl/src/harness/claude.rs`); submits with Enter.
- **Codex**: supported. Composer reader in
  `rs/agentctl/src/harness/codex.rs`; submits with Enter, or Tab while
  busy (Tab queues for the end of the turn). Codex 0.164's boxed layout
  (one status row under the shaded composer box) and 0.169's two-row
  footer (status row, then hint row) both parse: the status row is
  recognised by shape (`<model> <effort> · /path [· title]`), so an
  idle 0.169 pane accepts sends (#1553 codex-footer-composer, landed
  8a0d30a5).
- **Muse**: supported, by a separate path. The Muse adapter has no
  composer view; delivery goes through the verified-process editor
  reader in `rs/agentctl/src/client.rs` and the Muse submission paths in
  `rs/agentctl/src/subagents.rs` (#505 agentctl-muse-followup,
  #678 muse-bridge-latency, #1085 muse-send-staged-unsubmitted).
- **Unknown TUI**: partial. Bracketed paste, poll until the paste shows,
  Enter, poll until it leaves the bottom of the screen
  (`rs/agentctl/src/unknown_harness.rs`). Nothing about the program is
  known, so staging is judged from screen movement alone.
- **Headless**: supported for Claude Code only — the durable queue is
  the only drainer and the wrapper feeds the stream-json process
  (`rs/agentctl/src/headless/wrapper.rs`). Codex and Muse have no Rust
  headless adapter; Muse is in progress, #1234 headless-muse.

### Send: delivery receipts

A send reports `delivered` only on evidence: the prompt printed above
the composer, counted in the harness's own queue block, or recorded as
accepted — never merely because keys were sent (`SubmissionReceipt` in
`rs/agentctl/src/submission.rs`; the sender header every receipt rides
on is #609 sender-header-mandatory).

- **Claude Code, Codex**: supported, from the composer/transcript proof
  in `submit_verified`.
- **Muse**: supported, from a new user turn above the editor or Muse's
  journal recording `runtime.user_intent.accepted`
  (`rs/agentctl/src/harness/muse.rs`, `JournalWatch`).
- **Unknown TUI**: partial. A receipt needs the prompt shown as
  submitted; text that only left the bottom of the screen proves
  nothing, and the send stays possibly-submitted (#852, cited from
  `rs/agentctl/src/unknown_harness.rs`).
- **Headless**: supported — the wrapper logs turn acceptance and
  completion as typed events (`rs/agentctl/src/headless/events.rs`).

### Follow-up while working

What `agentctl send` may do while a turn runs (`FollowupPolicy` in
`rs/agentctl/src/harness/mod.rs`):

- **Claude Code**: partial — opt-in only (`OptIn`); a follow-up is typed
  only when the caller asked for delivery while working, and waits for
  a tool boundary.
- **Codex**: supported — always (`Always`); Codex steers on Enter and
  queues on Tab itself.
- **Muse**: supported — when verified (`WhenVerified`); Enter steers the
  running turn at once, proved from the screen or the journal.
- **Unknown TUI**: none (`Never`). A working pane takes no typed
  follow-up; the prompt waits in the queue.
- **Headless**: not applicable — every send is a queued turn.

### Stuck-paste / Enter backstop

The backstop for a prompt agentctl typed that the harness left
unsubmitted in its editor. Two layers: a delivery that finds such a
draft submits it before typing its own prompt (#720
agentctl-muse-stale-draft; `recover_typed_draft` in
`rs/agentctl/src/submission.rs`, Muse's own copy in
`rs/agentctl/src/subagents.rs`), and the watcher's idle sweep submits
one it finds later (#731 agentctl-paste-tail-flush, #1546
agentctl-unsubmitted-paste-backstop; `sweep_editor_drafts` in
`rs/agentctl/src/agent_watch.rs`). Attribution is deliberately strict:
only a draft that matches a prompt the queue records as typed is ever
submitted, under per-harness vouching (Muse `Recent`; Claude Code and
Codex `Unproven`, because their history keys recall the newest prompt)
in `rs/agentctl/src/stale_draft.rs`.

- **Claude Code**: supported (composer reader; the tall-draft unframed
  fallback landed with #1546).
- **Codex**: supported — the 0.169 footer misread that made the reader
  see a draft that is not there is fixed (#1553, landed 8a0d30a5), so
  Codex drafts attribute and flush like Claude Code's.
- **Muse**: supported (the Muse editor reader; #1546). A placeholder draft
  whose size exactly one typed delivery in its lookback has is attributed
  to that delivery even when later prompts followed it (#1732); a size
  two deliveries share still names neither.
- **Unknown TUI**: partial — report only from the per-harness sweep,
  which cannot read an unknown editor, so a stranded prompt there
  becomes a `draft: unreadable` blocked finding from that layer. The
  harness-agnostic watcher backstop below does submit a text-visible
  stuck paste there. The queued send itself retries with Enter only,
  never re-typing (#850).
- **Every harness, landed**: a harness-agnostic backstop that submits
  any text-visible stuck paste, so no per-harness reader is needed for
  the common case (#1560; `rs/agentctl/src/paste_backstop.rs`, pressed
  from the watcher's sweep). Placeholder-only pastes (`[Pasted Content
  N chars]`, `[Pasted text #K +M lines]`) show no screen text to match,
  so they stay with the per-harness readers. Since #1683 the same
  lower-region match also runs in the delivery path
  (`recover_typed_draft` in `rs/agentctl/src/submission.rs`): a
  delivery whose screen no composer reader recognises submits a typed
  draft from the region and notes its record recovered, and the chat
  bridge's stale-composer sweep reads the region when no composer
  parses, so an occupied but unparseable composer starts its clock —
  ours is submitted, a foreign draft is backed up and cleared only
  after the stale window. The region path presses Enter; on a busy
  Codex that steers, as Enter always does there.

This is the row the owner did not know diverged: the backstop grew up
Claude-first, reached Muse in #1546, and an unrecognised TUI still gets
a report instead of a rescue.

### Second Enter after delivery

One extra Enter about 1 s after every TUI typing attempt, on every outcome
(verified, unverified, stranded, paste-not-observed), before the pane lock
is released (#1691 agentctl-second-enter; `second_enter` in
`rs/agentctl/src/submission.rs`, mirrored in `py/agentctl/submission.py`).
It presses only on an empty composer or on our own staged text or
post-type placeholder, skips approval/permission/picker/dialog screens
and foreign drafts (`second_enter_skipped` on the queue record), and a
flush is receipted as `submitted by second_enter` with `second_enter_at`
stamped. Bare Enter on an empty composer was verified harmless live for
Claude Code, Codex and Muse. Opt out: `AGENTCTL_SECOND_ENTER=0`. Unknown
TUI gets none (no composer model); headless has no composer. Muse, which
never runs `submit_verified`, gets the matching delayed re-check instead
(#1732): after a verified steer, if its editor again holds exactly the
prompt just sent, one Enter (`recheck_muse_submitted` in
`rs/agentctl/src/subagents.rs`); foreign text and approval dialogs are
left alone, and the verified outcome stands.

### Composer / draft detection

Reading the composer's current text — to refuse appending to a person's
draft, and to feed the backstop above.

- **Claude Code**: supported (`composer_view` in
  `rs/agentctl/src/harness/claude.rs`, including the unframed tall-draft
  fallback).
- **Codex**: supported (`rs/agentctl/src/harness/codex.rs`). The
  composer ends at its own blank boundary: 0.164's boxed single status
  row and 0.169's two-row footer (status row parsed by shape, then the
  hint row) are footer, never draft text. Rows under the composer that
  no rule classifies read as composer-unknown beneath a
  placeholder-only composer (never a draft that refuses a send), and
  as the draft's own paragraphs beneath a composer holding real text
  (#1553, landed 8a0d30a5).
- **Muse**: supported, outside the adapter — the editor reader in
  `rs/agentctl/src/client.rs` (`muse_verified_process_editor_rows` and
  friends), used by delivery, status, and the sweep.
- **Unknown TUI**: none. There is no composer to read; readiness is
  screen quiet for two seconds with clock digits and spinner glyphs
  masked (`rs/agentctl/src/unknown_harness.rs`,
  `rs/agentctl/src/screen_heuristics.rs`).
- **Headless**: not applicable.

### Busy / idle detection

`agentctl status` and the watcher start from the terminal host's state
and let the screen overrule it where the host's rules cannot see
(`settle_herdr_state`, `rs/agentctl/src/harness/mod.rs`; turn and queue
reading in `rs/agentctl/src/turn_state.rs`; process and transcript
progress in `rs/agentctl/src/work_state.rs` and
`rs/agentctl/src/agent_watch.rs`, #570 agent-progress-monitoring).

- **Claude Code**: supported — the running row, background shell/agent
  counts in the footer, and the scrolled-up pill are all read
  (`background_work`, `scrolled_below` in
  `rs/agentctl/src/harness/claude.rs`); a scrolled-up view is never
  judged stalled, idle, or finished.
- **Codex**: supported — agentctl itself settles an `unknown` host state
  to idle for an empty composer with a recognised footer
  (`settle_herdr_state` in `rs/agentctl/src/harness/codex.rs`), because
  the host's bundled Codex rules never report idle.
- **Muse**: supported — the running row in any of its three glyphs
  overrules the host's report (`muse_verified_pane_status` in
  `rs/agentctl/src/client.rs`).
- **Unknown TUI**: partial — quiet screen only; no turn timer, no queue
  block, no transcript.
- **Headless**: supported from the event log instead of a screen
  (`rs/agentctl/src/headless/watch.rs`).

### Interrupt a running turn

- **Claude Code, Codex, Muse**: supported for interactive agents —
  Escape, with guards: nothing is pressed while an approval prompt or a
  composer draft shows, because Escape would answer the one or merge
  the other (`interrupt_with` in `rs/agentctl/src/subagents.rs`). The
  `agentctl send --urgent` path interrupts and submits the queued input
  the same way. Muse additionally steers with Enter, so an urgent Muse
  delivery often needs no interrupt at all.
- **Unknown TUI**: partial — the same Escape path runs, judged by the
  generic screen rules; a turn that does not stop reads as unknown
  rather than interrupted.
- **Headless**: supported for Claude Code — `agentctl interrupt` asks
  the wrapper over its control socket; queued inputs stay queued
  (`rs/agentctl/src/headless/control.rs`). The `interrupt` command
  itself is headless-only; interactive interrupt rides on `send
  --urgent` and the watcher's auto-deliver policy.

### Approvals relay to chat

- **Claude Code, Codex, Muse, Unknown (interactive)**: none. agentctl
  detects that a pane waits at an approval, permission, or trust prompt
  and reports it — the watcher as a blocked episode, `agentctl status`
  as blocked — but the answer is given in the pane by a person. No TUI
  approval is posted to chat and none is answered from chat. Muse's
  peer-approval dialog is detection only, by design (#974
  muse-peer-approval-prompt).
- **Headless (Claude Code)**: supported. The wrapper holds the prompt,
  logs `approval.requested`, a bridge serving the agent posts it to the
  chat channel, and the owner answers `approve ID` / `deny ID` there or
  `agentctl approve|deny` at a terminal (#671
  headless-approval-surface; `rs/agentctl/src/headless/approval.rs`,
  `rs/agentctl/src/chat_service.rs`). An unanswered prompt is denied
  when its timeout ends.

### Forms / questions (`prompts`, `answer`)

`agentctl prompts` inspects a blocking question and `agentctl answer`
answers a recognised form, per harness in
`rs/agentctl/src/harness_tui/`.

- **Claude Code**: supported — single-question selection forms, option
  answers only; free-text forms are refused
  (`rs/agentctl/src/harness_tui/claude.rs`).
- **Codex**: supported — question forms including free text through the
  "None of the above" option, plus the rate-limit model-switch form
  (`rs/agentctl/src/harness_tui/codex.rs`).
- **Muse**: none — Muse forms are classified so status can say
  blocked, but `answer_plan` always refuses: "Muse forms are not
  answered by agentctl; answer them in the pane"
  (`rs/agentctl/src/harness_tui/muse.rs`, offered only to follow-up
  delivery, never to `prompts`/`answer`).
- **Unknown TUI**: none — no classifier exists, so `prompts` and
  `answer` have nothing to read.

### Trust dialogs

The folder/workspace trust screen each harness shows on first launch in
a new directory (`trust_prompt` in `rs/agentctl/src/harness/mod.rs`). A
managed send checks for it before typing into an idle pane.

- **Claude Code**: supported (`rs/agentctl/src/harness/claude.rs`).
- **Codex**: supported (`rs/agentctl/src/harness/codex.rs`).
- **Muse**: partial — Muse's trust phrases are in the cross-harness
  approval table the watcher scans, so a Muse trust prompt is reported,
  but the Muse adapter has no `trust_prompt` check of its own, so the
  managed-send guard does not name it. Muse 1.4.24's plugin-hooks trust
  dialog ("Review plugin hooks") is recognised as its own startup state
  (#1893 agentctl-launch-failed-but-live): launch readiness waits while
  it shows and names it (`waiting: hooks trust`) instead of timing out
  as a missing idle composer, status reports the same, and nothing ever
  answers it (`muse_hooks_trust_prompt` in `rs/agentctl/src/client.rs`,
  `py/agentctl/client.py`; `agentctl promote` repairs the
  `launch_failed` record once a person clears it).
- **Unknown TUI**: none.

### Headless mode itself

`agentctl start --mode headless` in the Rust edition runs exactly one
harness: `HARNESSES = ["claude"]` in `rs/agentctl/src/headless/mod.rs`,
Claude Code over stream-json (#565 headless-coordinator). Codex headless:
none in this edition. Muse headless: in progress, #1234 headless-muse
(mu-musehl) — a Muse adapter beside `rs/agentctl/src/headless/claude.rs`
under the same wrapper, so queue, event log, `tail`, `console`, and
approvals behave as a Claude headless agent's do. The Python edition's
headless workers are a different mechanism; see
[The Python edition](#the-python-edition).

### Transcript reading

The watcher and `status` read each harness's own transcript for the
last event, model output, open request, and tool call in flight
(`transcript_path`, `read_transcript`, `transcript_model` in
`rs/agentctl/src/harness/mod.rs`; #574, #1340 agent-model-unknown).

- **Claude Code**: supported — the session JSONL under the projects
  directory, sub-agent transcripts included
  (`rs/agentctl/src/harness/claude.rs`).
- **Codex**: supported — the rollout files, sub-agent threads included
  (`rs/agentctl/src/harness/codex.rs`).
- **Muse**: supported — the session journal, sub-agent sessions
  included (`rs/agentctl/src/harness/muse.rs`).
- **Unknown TUI**: none — no transcript is known or read; the watcher
  sees only the screen and the process tree.
- **Headless**: supported — the typed event log replaces the transcript
  (`rs/agentctl/src/headless/events.rs`, `tail.rs`).

### `--trace-thread` harvesting

The chat bridge harvesting one agent's transcript into a daily trace
thread (`agentctl chat run --trace-thread`, #714/#832; collector in
`rs/agentctl/src/chat_trace.rs`, driver in
`rs/agentctl/src/chat_service.rs`). This is a different capability from
transcript *reading* above: it posts the agent's non-reply text to chat,
where vibe-talk classes it into the Transcripts view (#1608). It needs a
decodable transcript, so its coverage follows the decoder list, not the
screen.

- **Claude Code**: supported — harvested from the session transcript
  through the #710 reader (`rs/agent-harness`).
- **Codex**: supported — the same reader over the rollout files.
- **Muse**: supported — the same reader over the session journal.
- **Unknown TUI**: none — no transcript is known, so there is no
  outside-the-fence text to collect; its chat replies still work.
- **Headless**: supported (typed events) — the typed event log stands in
  for a transcript file.

### `tail` / `console` / `read`

- **Interactive (all three known TUIs)**: partial — `agentctl read`
  prints a terminal snapshot; `read --output last` and `since_turn`
  refuse, because an interactive agent exposes no transcript boundary
  (`rs/agentctl/src/cli.rs`). `tail` refuses: only headless agents write
  an event log.
- **Unknown TUI**: partial — the same snapshot `read`; nothing else.
- **Headless (Claude Code)**: supported — `tail` follows the event log,
  `console` adds the owner's input line, approval answers, and
  `/interrupt` (#670 agentctl-console; `rs/agentctl/src/console.rs`).

### Session identity

How the record learns the harness's native conversation id (#741
restore-native-sessions).

- **Claude Code**: supported, strongest form — agentctl assigns the id
  itself before launch (`assigns_session_id`, `--session-id` in
  `rs/agentctl/src/harness/claude.rs`), so the record knows it before
  the harness starts.
- **Codex**: supported — discovered from the rollout files the process
  holds open (`held_sessions` in `rs/agentctl/src/harness/codex.rs`).
- **Muse**: supported — discovered from the session lock the runtime
  holds open (#916 muse-native-session-id; `held_sessions` in
  `rs/agentctl/src/harness/muse.rs`). A fresh Muse start mints its own
  id, which agentctl records at anchor; agentctl cannot name a Muse
  conversation in advance.
- **Unknown TUI**: none — no id is assigned, discovered, or recorded.
- **Headless**: supported — the wrapper owns the session id and resumes
  it across restarts.

### Takeover / handback

Moving one conversation between headless and interactive presentation
(`rs/agentctl/src/subagents.rs`, headless takeover tests).

- **Headless Claude Code → interactive Claude Code**: supported —
  `agentctl takeover` stops the wrapper and opens `claude --resume
  SESSION` in a new tab; `handback` ends it and restarts the wrapper on
  the same session. Inputs sent meanwhile stay queued.
- **Codex, Muse**: none — there is no headless session to take over,
  and no interactive-to-headless conversion. Muse's #1234 acceptance
  requires its session identity to round-trip through takeover/handback
  once headless Muse exists.
- **Unknown TUI**: none.

### Restore / revive (launch replay)

After a restart or reboot, relaunching each agent with its recorded
arguments and conversation (`split_conversation`, `fresh_arguments`,
`replay_arguments` in `rs/agentctl/src/harness/mod.rs`; replay in
`rs/agentctl/src/subagents/launch_spec.rs`, revive in
`rs/agentctl/src/subagents/revive.rs`, team restore in
`rs/agentctl/src/restore.rs`).

- **Claude Code**: supported — selectors (`--session-id`, `--resume`,
  `--continue`, `-r`, `-c`) are split, refused when ambiguous, and
  replayed exactly.
- **Codex**: supported — a leading `resume ID` is split and replayed; a
  selector with no id (`--last`) is refused rather than guessed.
- **Muse**: partial — a trailing `resume ID` replays, but a fresh start
  cannot resume a conversation agentctl names (#741): revive starts
  the recorded arguments and Muse mints a new session.
- **Unknown TUI**: none — launch specs cannot replay an unrecognised
  harness (`split_conversation` returns `None`; replay errors).
- **Headless**: supported — the wrapper resumes its recorded session;
  restore skips live wrappers and never repeats a published revive.

### Effort control

`--effort` at launch and `agentctl set-effort` on a live agent (#1216
agentctl-effort-option; levels in `rs/agentctl/src/profiles.rs`,
launch arguments per adapter). The level sets themselves diverge:

- **Claude Code**: supported — `low, medium, high, xhigh, max` via
  `--effort`. There is no `none`, `minimal`, or `ultra`.
- **Codex**: supported — `minimal` through `ultra` via
  `--config model_reasoning_effort=…`; Codex validates the level
  against the chosen model, so agentctl does not narrow the list.
- **Muse**: supported — `none` through `ultra` via
  `--reasoning-effort`; `ultra` additionally needs its experimental
  gate environment variable, which agentctl supplies
  (`rs/agentctl/src/subagents.rs`).
- **Unknown TUI**: none — no structured effort mapping exists, and
  `set-effort` refuses the harness by name.
- **Headless**: supported for Claude Code — recorded at once, applied
  at the next wrapper launch; the console applies it at the next turn
  boundary (#1396 agentctl-console-effort-model).

A live TUI agent applies a recorded effort at a safe point through the
harness control where one exists, otherwise on restart `--resume`; a
mid-turn agent is never interrupted for it without `--urgent`.

### Model selection

`--model` at launch, a profile's model, and `agentctl set-model`
(#1396; validation in `rs/agentctl/src/profiles.rs`).

- **Claude Code, Codex, Muse**: supported — structured launch passes
  the model through (`launch_arguments` per adapter); agentctl keeps no
  model allowlist of its own, so the harness accepts or refuses the
  name.
- **Unknown TUI**: none — a harness with no structured launch takes no
  structured model, and validation refuses it.
- **Headless**: supported for Claude Code, applied at the next wrapper
  launch or turn boundary.

### Launch failure hints

When a launch fails, the pane screen it left behind can name the cause
and remedy (`HarnessAdapter::launch_failure` in
`rs/agentctl/src/harness/mod.rs`).

- **Codex**: supported on the native Herdr start path — a screen
  showing `daemon executable not found` makes the launch error name
  the `--no-daemon` relaunch (for example `--harness-arg=--no-daemon`);
  the flag is reported, never added silently (#1553, landed 8a0d30a5).
  Other launch backends keep the generic launch error.
- **Claude Code, Muse, Unknown TUI, Headless**: none — the generic
  launch error stands.

### Launch-env replay

Environment recorded at launch (names in the record; values stay in
private profiles) and re-applied on relaunch
(`environment_names` in `rs/agentctl/src/subagents/launch_spec.rs`).

- **Claude Code, Codex, Muse**: supported — a fresh-tab restart
  re-applies the recorded environment. A same-pane restart refuses when
  a launch environment is recorded, because the tab's original shell
  environment is gone and typing values would put them in scrollback;
  it names the fresh-tab restart instead.
- **Unknown TUI**: partial — the record and refusal behave the same,
  but no harness-specific launch applies the values beyond the recorded
  arguments.
- **Headless**: supported — the wrapper's launch environment is part of
  its recorded launch spec.

### Permission bypass at launch

Every agent runs on bypass permissions (owner rule, 2026-11-17; #1822
agentctl-profile-requires-bypass). An interactive profile whose argv
carries no bypass flag refuses to load, and an interactive start,
revive, restart, restore, or turnover relaunch from arguments with no
flag refuses the same way; the refusal names the profile (or agent),
the harness, and the exact flag to add (`permission_bypass_flag` /
`require_permission_bypass` in `rs/agentctl/src/profiles.rs`, mirrored
in `py/agentctl/profiles.py`; launch checks in `rs/agentctl/src/cli.rs`,
`rs/agentctl/src/subagents/revive.rs`, `rs/agentctl/src/subagents.rs`,
and `py/agentctl/cli.py`, `py/agentctl/revive.py`).

- **Claude Code**: supported — `--dangerously-skip-permissions` or
  `--permission-mode bypassPermissions` (either spelling, split or
  `=`-joined; both verified against `claude --help`).
- **Codex**: supported — `--dangerously-bypass-approvals-and-sandbox`
  or `--yolo` (the first verified against `codex --help` on the filing
  host; `--yolo` is accepted as the documented alias).
- **Muse**: supported — `--approval-mode never` or `--yolo`
  (both verified against `muse --help`: `--yolo` disables approval
  and sandbox).
- **Unknown TUI**: n/a — agentctl knows no launch arguments for the
  program it drives, so there is no flag to require.
- **Headless**: supported — headless profiles carry no raw argv for
  codex/agy (empty by rule), but headless Claude has no carve-out
  (coordb, #1822): a headless Claude start, `run-headless`, revive, or
  restart whose harness args lack `--permission-mode bypassPermissions`
  (or `--dangerously-skip-permissions`) refuses with the same message
  (`start_headless` and the wrapper launch in
  `rs/agentctl/src/subagents/headless.rs`). `--permission-prompts host`
  keeps working alongside the bypass, so hook "ask" decisions still
  route to chat (`effective_arguments` in
  `rs/claude-stream-session/src/protocol.rs`); the Python foreign runner
  appends the Codex bypass flag itself
  (`py/agentctl/foreign/agent_runner.py`), and this edition runs no
  headless Claude at all.
- **agentcloud** (not a column; see [agentcloud](#agentcloud)): no
  bypass flag known — `agentcloudctl create` offers only
  `--dangerously-skip-sandbox`, a sandbox switch, not permissions, so
  agentctl requires nothing and invents no flag; the profile listing
  carries `permission_bypass_warning` ("no bypass flag known") instead.

### Chat-bridge delivery

A chat request typed into the bridged agent by `agentctl chat run`
(`rs/agentctl/src/chat_service.rs`, `rs/agentctl/src/chat_runtime.rs`).
The bridge delivers through the same queue and submission paths as
`agentctl send`, so its per-harness truth is the send rows above, plus:

- **Claude Code, Codex, Muse**: supported, including delivery while
  working under each harness's follow-up policy.
- **Unknown TUI**: partial — the generic paste-and-Enter path; an
  uncertain delivery is reported uncertain to the chat requester, never
  assumed.
- **Headless (Claude Code)**: supported — the wrapper drains the queue;
  bridge raw messages keep their raw mark there (#1223
  bridge-raw-headless).

### Watcher notices (DONE / BLOCKED / idle / stalled)

`agentctl watch-agents` classifies every watched agent each pass and
queues notices to its coordinator: a `DONE:`/`BLOCKED:` verdict
(child-done), idle without a verdict, stalled, approval prompt, dead,
turn-queued, tool-blocked, stream-silent, and more (`EpisodeKind` in
`rs/agentctl/src/agent_watch.rs`). Verdicts are read from the last reply
on screen, whatever harness drew it; progress evidence is per-harness.

- **Claude Code, Codex, Muse**: supported — screen verdicts plus
  transcript and process progress (#570, #574), including the
  stuck-paste sweep of the backstop row.
- **Unknown TUI**: partial — screen verdicts and quiet/idle only; no
  transcript episodes (untimed-tool, stream-silent by transcript) can
  fire.
- **Headless**: none — the watcher skips headless and agentcloud agents
  by record; their supervision is the wrapper's own health reporting.
- **agentcloud**: none — skipped the same way; it has no local screen
  at all (`rs/agentctl/src/harness/agentcloud.rs`).

### Raw messages (no sender header)

An owner's chat message marked raw is typed exactly as written — no
sender header, no reply instructions — so a slash command runs as
terminal input (#1136 raw-message-bridge; `rs/agentctl/src/chat_runtime/raw.rs`).
Every prompt `agentctl send` delivers otherwise opens with a mandatory
sender header (#609 sender-header-mandatory;
`rs/agentctl/src/sender_header.rs`).

- **Claude Code, Codex, Muse**: supported — typed through the harness's
  normal submission path, header omitted.
- **Unknown TUI**: partial — typed through the generic path; the same
  evidence limits as its receipts apply.
- **Headless (Claude Code)**: supported — a raw item is queued verbatim
  under the one shared definition (#1223).

### Goal commands

- **Claude Code, Codex, Muse**: supported — a `/goal` command is typed
  raw and proved from the answer row each harness prints, since none of
  them echoes it as an ordinary user turn
  (`rs/agentctl/src/goal_command.rs`). Codex additionally exposes its
  native goals over the app-server protocol (`agentctl codex-goal`,
  `rs/agentctl/src/codex_goal.rs`) — Codex only.
- **Unknown TUI**: none — no answer row is known, so no proof exists.
- **Headless**: partial — goals ride as ordinary queued turns; there is
  no screen answer to prove them with.

### Skill installation

`agentctl` installs its own skill into each harness
(`rs/agentctl/src/skill_install.rs`).

- **Claude Code, Codex**: supported — copied into the harness's skill
  directory (`.claude`, `.codex` via `skill_directory`).
- **Muse**: supported — installed through Muse's own skill manager
  (`muse skills install`), with its receipt checked.
- **Unknown TUI**: none — there is no known skill directory, and the
  install refuses the harness by name.

### Boxed launch (`--slot`, userns)

Launching into a wrkslots slot box (`agentctl start --slot …`;
isolation in `rs/agentctl/src/subagents.rs`, boxed form per adapter in
`rs/agentctl/src/harness/mod.rs`). Inside a user-namespace box every
harness launcher's own sandbox entry fails (it sets its effective UID
to root; the box maps no inner root), so a boxed launch must run the
harness without that step — the box is the sandbox.

- **Muse**: supported (#1583 boxed-muse-lane-identity). The adapter's
  boxed form runs the runtime beside the launcher with its shell
  sandbox disabled (`boxed_executable`, `boxed_arguments` in
  `rs/agentctl/src/harness/muse.rs`; launch in
  `rs/agentctl/src/client.rs`). A missing runtime fails the launch
  loudly, naming the path looked for. The Python edition mirrors this
  (`boxed_runtime`, `boxed_arguments` in `py/agentctl/client.py`).
  Verified live: start, send, read, stop in a user-namespace box.
- **Claude Code**: none under userns — its native binary performs the
  failing step itself, and its sandbox-disable flag refuses inside
  the box view. Limits-only `cgroup` boxing works (no mount view).
  Mount-sandboxed Claude Code uses root isolation (the relay below).
- **Codex**: none under userns — same launcher failure. A runtime
  beside its launcher exists, but Codex launches through the terminal
  host, not a pane command agentctl owns; root-relay boxing is the
  current mount-sandboxed route for it.
- **Root isolation (all three known TUIs)**: supported as a relay —
  the boxed harness line runs in the pane directly behind the slot's
  root launcher, and agentctl owns its lifecycle state
  (`RELAY_HARNESSES` in `rs/agentctl/src/subagents.rs`). Muse joined
  the relay in #1583: its readiness is the verified-process editor
  proof on the plain screen (`start_relay_agent` in
  `rs/agentctl/src/client.rs`). Muse relay delivery is partial: a
  relayed Muse starts, reports state, and stops, but `send` is
  blocked (no composer model for a relayed Muse yet). An instantly
  failing boxed command takes its exec'd pane with it; the relay
  launch now names that instead of a bare pane refusal.
- **Unknown TUI**: none — no boxed form is declared.

### Transient-failure recovery; stalled-goal resume

The two smallest divergences, kept as rows so they cannot hide:

- **Transient recovery** (an opt-in continue prompt after a verified
  availability failure): **Claude Code and Codex only**
  (`transient_recovery` in their adapters). Muse and Unknown: none.
- **Stalled-goal resume** (typing the harness's native resume command
  when its goal stalls): **Codex only** (`/goal resume`,
  `stalled_goal_resume` in `rs/agentctl/src/harness/codex.rs`). Every
  other harness: none.

## Where the owner would be surprised

1. **The stuck-paste backstop is not harness-agnostic yet.** It grew up
   Claude-first; Muse got it in #1546; an unrecognised TUI gets a
   report, not a rescue. #1560 is making it general now.
2. **No TUI approval reaches chat.** Only a headless Claude agent's
   approvals are posted to chat and answered there. A Claude, Codex, or
   Muse pane waiting at an approval is reported, and a person answers
   in the pane.
3. **`agentctl answer` does not answer Muse.** Muse forms are detected
   so status can say blocked; answering them is refused by design.
4. **Headless means Claude.** In the Rust edition that the launchers
   run, `start --mode headless` starts Claude Code only. Codex has no
   headless adapter there, and Muse's is in progress (#1234).
5. **Effort levels are three different sets.** `ultra` exists for Codex
   and Muse but not Claude Code; `none` exists only for Muse; a level
   copied from one harness's profile to another can be refused.
6. **Muse session identity is discovered, not assigned.** Claude Code
   gets its id before launch; Codex and Muse ids are found afterwards
   from files the process holds open, and a Muse conversation cannot
   be resumed by an id agentctl chose.
7. **Codex idle is agentctl's own call.** The terminal host's bundled
   Codex rules never report idle, so agentctl settles it from the
   composer and footer — the same footer whose 0.169 form currently
   breaks Codex sends (#1553).

## The Python edition

The Python edition (`py/agentctl/`) still ships and is kept in
differential lockstep for the interactive core, but it is not identical:

- Verified submission covers **Claude Code and Codex only**
  (`VERIFIED_HARNESSES` in `py/agentctl/submission.py`); its form
  adapters are likewise Claude and Codex only
  (`py/agentctl/harness_tui/`). Muse delivery refinements land in the
  Rust edition first.
- The lower-region recovery for an unparseable composer (#1683) covers
  the prompt being delivered (`_region_match` and `_flush_region_draft`
  in `py/agentctl/submission.py`, Claude Code and Codex), as does the
  delayed re-check after a verified submission (`_recheck_after_verify`
  there, `recheck_submitted` in the Rust edition); recovering any
  typed draft, and the stale-composer clear, remain Rust-only. The
  pane-lock hold bound is summed from the phase timeouts in both
  editions (`SubmitTimeouts::hold_bound`, `hold_bound_seconds`). The
  doctor and status warning for a terminal another registry also
  records (`terminal-shared-across-registries`, #1683) is mirrored
  here: both editions read the sibling registries named by
  `restore.json`.
- Its headless workers are a different mechanism — resumable Codex, AGY,
  and Muse structured turns driven per turn (`py/agentctl/foreign/`),
  not the Rust wrapper's persistent Claude process. The user guide's
  edition table says this plainly; the matrix's Headless column is the
  Rust meaning.
- Where the two editions disagree, this matrix records the Rust
  edition, and the disagreement itself is a cell to fix, not a detail
  to omit.

## agentcloud

A remote agentcloud session has no local screen at all: it takes prompts
through its journal, and the driver inside the session (Claude Code,
Codex, or Muse) is chosen at creation (`rs/agentctl/src/harness/agentcloud.rs`).
It supports send, status, read, wait, stop, attach, pause, resume, and
final-output; it has no composer, no turn to interrupt, no takeover, and
the watcher skips it. It is deliberately not a matrix column — its
support is defined by that list, not by the screen-driven rows — but any
change to it still updates this section under the same rule.

## Claims the code does not back

Docs and skills checked against the code above on 2026-11-15. Each claim
below is stated, or plainly implied, somewhere a reader trusts, and the
code does less:

1. The agentctl skill lists `agentctl answer NAME --option N` among the
   ordinary commands with no harness qualification
   (`py/agentctl/AGENTCTL_SKILL.md`). For Muse it always refuses; for
   an unknown TUI there is nothing to answer. The skill now points here
   for the per-harness truth.
2. The skill's `agentctl send` summary describes delivery while working
   as one behaviour ("interrupts the turn and submits the queued input
   instead of anyone pressing Escape and Enter by hand"). In the code
   the three harnesses do three different things — Claude Code waits
   for a tool boundary unless opted in, Codex queues itself, Muse
   steers — and an unknown TUI takes no follow-up at all.
3. The Python CLI's `start --mode headless` help names Codex and Muse
   headless alongside the Rust headless agents, as if `--mode headless
   --harness muse` were one feature with two editions. They are two
   mechanisms with different contracts (see above); in the Rust
   edition the same command refuses every harness but Claude.
4. Nothing before this document said the stuck-paste backstop, TUI
   approval relay, and `answer` for Muse were missing. The absence was
   discoverable only by reading adapters — which is the gap this
   matrix exists to close.

No guide sentence was found that claims a capability the code lacks
outright (a named command that does not exist, a flag that is refused
everywhere); the divergences above are omissions and edition blur, and
they are recorded here rather than silently corrected in the guides, so
the next reader can watch them close.

## Keeping this current

The rule at the top is the whole maintenance plan: the commit that
changes a harness's support changes this file. Reviewers check for it
the way they check for a test. When a lane's work changes a cell —
#1553 (Codex footer), #1560 (general backstop), #1234 (headless Muse)
as of this writing — whichever lands later updates the row in its own
commit and says so in its issue's Notes.





