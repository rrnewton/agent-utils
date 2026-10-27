# Harness adapters: every per-harness special case, and where it lives

#574 harness-adapters, 2026-10-27. Working notes, not user documentation.

agentctl drives coding-agent harnesses it does not control (Claude Code, Codex, Muse, remote
agentcloud sessions) through their terminal screens, command lines and files. Each of those
facts belongs to one harness, often to one release of it. Before this change they were spread
across `submission.rs`, `agent_watch.rs`, `turn_state.rs`, `client.rs`, `agent.rs`,
`subagents.rs`, the launch-spec and revive code, the chat bridge and the Python edition, mostly as
`if harness == "codex"` branches and string constants.

This note lists them all, says which are cross-harness and which are harness-specific, and says
which work around a bug or gap in a program we do not own. The Rust edition now has one module per
harness behind one trait (`rs/agentctl/src/harness/`), with the cross-harness logic left where it
was, asking the adapter:

| Module | What it holds |
|---|---|
| `harness/mod.rs` | `HarnessAdapter` (launch, screen, follow-up policy, transcript), `ScreenMarkers` (each harness's phrases and glyphs), and the cross-harness unions the screen readers use when they do not know which harness drew a screen (`approval_patterns()`, `queue_hints()`, `reply_bullets()`, `user_turn_glyphs()`, `tool_output_glyphs()`, `running_glyphs()`, `composer_placeholders()`), plus `WORKING_MARKER` (`esc to interrupt`), the one phrase every screen harness shares. |
| `harness/claude.rs` | Claude Code. |
| `harness/codex.rs` | Codex. |
| `harness/muse.rs` | Muse (its ruled editor stays in `client/muse_editor.rs`, its forms in `harness_tui/muse.rs`). |
| `harness/agentcloud.rs` | agentcloud: no screen, journal input, skipped by the watcher. |
| `harness/transcript.rs` | Cross-harness transcript plumbing: locating a transcript (session id, or the files the harness process holds open), reading its tail, and the `Activity` the per-harness readers fill in. |
| `harness_tui/` (unchanged) | Form classification and answer keys, per harness; each adapter names its own. |

Line numbers below are at the commit that introduced the modules. "Py" is the Python edition's
copy, which the cross-language differential tests keep in step; it was not restructured.

## Inventory

Legend for **Scope**: *X* cross-harness (holds for every harness, or is the union of per-harness
tables); *C* Claude Code; *Cx* Codex; *M* Muse; *A* agentcloud; *H* Herdr glue (a fact about
Herdr's view of a harness). **Upstream** marks a workaround for a bug or gap in a program we do
not own; the numbered drafts are below.

### Screen: composer, submission keys, running turn, queued input

| # | Special case | Scope | Rust | Py | Upstream |
|---|---|---|---|---|---|
| 1 | Composer layout: the `❯` row between two horizontal rules; a labelled top border (`──── ultracode ─`) tried only when two plain rules do not frame one | C | `harness/claude.rs:132` (`view_framed`), helpers `submission.rs` (`is_rule`, `is_labelled_rule`, `composer_from`) | `submission.py:340` | |
| 2 | Composer layout: first row opens with `›` (older) or `»` (v0.159.1, which keeps `›` for selection lists), footer row indented two spaces | Cx | `harness/codex.rs:167`, `:181` | `submission.py:342` | release churn |
| 3 | Muse's ruled editor, its header (`Muse Code X.Y.Z`, scrolled off by 1.4), prompt markers `❯`/`›`, turn-end markers `◆ ❯ › ◇ ◈` | M | `client/muse_editor.rs:26`, `:30` | `client.py:146` | |
| 4 | Which harnesses have a verified composer model (Claude Code, Codex) | X | `HarnessAdapter::verified_submission`; `submission.rs:191` | `submission.py` `VERIFIED_HARNESSES` | |
| 5 | Submission key: Enter, except a busy Codex showing `tab to queue message`, which gets Tab (Enter would steer the running turn) | Cx | `harness/codex.rs:123`, `:163` | `submission.py:511` | see draft 6 |
| 6 | A dropped submission key: the key is retried with doubling waits while the exact text stays staged, never re-typed | X | `submission.rs` `submit_verified` (`FIRST_RETRY`, `MAX_RETRY`) | `submission.py` | draft 4 |
| 7 | A paste may show late (seen 3 s, then found later): wait up to `STAGE_TIMEOUT` (60 s) before any key | X | `submission.rs:40` | `submission.py` | |
| 8 | Herdr reports a freshly launched harness ready before it draws its composer: wait up to `COMPOSER_WAIT` (15 s) before typing | H | `submission.rs:47` | `submission.py` | draft 3 |
| 9 | Paste placeholders: Claude Code `[Pasted text #N +M lines]`, Codex `[Pasted Content ...]` count as the staged prompt | C, Cx | `submission.rs:532`, `:539`; `harness/codex.rs:220` | `submission.py` | |
| 10 | `paste again to expand` replaces Claude Code's status row after a paste, hiding `esc to interrupt`: a screen ending in it cannot say whether a turn runs | C | `harness/claude.rs:28`; used `submission.rs:400` | `submission.py:97` | |
| 11 | Running-turn marker `esc to interrupt` in the bottom 16 rows (Claude footer and spinner row, Codex progress row, Muse running row) | X | `harness/mod.rs:273`; `submission.rs:400`, `:440` | `submission.py:89` | |
| 12 | Queued-input hints: Claude `Press up to edit queued messages`; Codex `edit last queued message`, `Queued follow-up inputs` | C, Cx | `harness/claude.rs:13`, `harness/codex.rs:16`; union `harness/mod.rs:230` | `submission.py:85` | |
| 13 | Codex holds mid-turn messages under `Messages to be submitted after next tool call` (Enter) or `... at end of turn` / `Queued follow-up inputs` (Tab); previews read back as printed evidence | Cx | `harness/codex.rs:171`, `:383` | `submission.py` | draft 6 |
| 14 | A prompt a working Claude Code never accepts is withdrawn (cleared) and reported not typed | C | `submission.rs:1042`; `subagents.rs:3846` | `agent.py:2148` | |
| 15 | Codex idle-composer proof: empty composer, recognised footer (`? for shortcuts`, `NN% context left`, or a path footer), no live timer row, none of 22 refusal phrases (dialogs, queue hints, trust, approvals), not the grouped search menu | Cx | `harness/codex.rs:220`, `:277`, `:322` | `client.py:1689` | draft 1 |
| 16 | Running-turn row: column-0 glyph (Codex `•`/`◦`, Muse `◆`/`◇`, Claude spinner frames `✻✽✶✳✢*`) then a parenthesised group with an elapsed timer and `esc to interrupt` or a token counter; Muse's timer times the step, not the turn | X (per-harness glyphs) | `turn_state.rs:149`, `:168`; glyphs `harness/*.rs` `running_glyphs` | `client.py:146` | |
| 17 | Queued-input blocks: Codex headings with `↳` items (wrapped and `…`-elided), Muse `◆ Queued input` with `└` items, Claude's hint only (one or more) | C, Cx, M | `turn_state.rs:213`, `:247`, `:270`; `HarnessQueue` `turn_state.rs:39` | | draft 6 |
| 18 | Composer draft for the watcher and interrupt: last `»`/`›`/`❯` row; placeholders (Codex's rotating suggestions, Claude's queue hint, `Type your message`, attribution unrecorded) are not drafts | X (per-harness placeholders) | `turn_state.rs:327`; tables `harness/*.rs` | | |
| 19 | Timer masking: the running row's timer, spinner glyph and Claude's rotating verb are not progress; its token counter is | X | `turn_state.rs` `without_timer`; `agent_watch.rs:418` | | |
| 20 | Slash commands keep the native (unverified) path: they open menus | X | `submission.rs:191` | `submission.py` | |

### Screen: prompts, verdicts, loops, background work, scrolling

| # | Special case | Scope | Rust | Py | Upstream |
|---|---|---|---|---|---|
| 21 | Approval and trust phrases (lowercased, bottom 12 rows): Claude `do you want to proceed`, `yes, i trust this folder`; Codex `would you like to run`, `allow command?`, `do you trust`; Muse `approval required`, `requires approval`, `approve this`, `trust this workspace`, `workspace trust`. Attribution follows the source comments; the union is what is matched | C, Cx, M | tables `harness/*.rs`; `agent_watch.rs:288` | | |
| 22 | Folder-trust dialogs: Claude `Quick safety check: ...` + `Yes, I trust this folder` (a managed send also requires `No, exit`); Codex `Trust this folder?` + `Trust and continue` | C, Cx | `harness/claude.rs:31`, `harness/codex.rs:40`; `subagents.rs:4115`; `client.rs` `relay_trust_prompt` | `client.py:671` | |
| 23 | Reply bullets `●` (`⏺` older), `◆`, `•`; user-turn echo `❯`/`>`/`›`; tool output under `⎿`/`└`: the verdict is a prose line of the last reply, not tool output | X (per-harness glyphs) | `agent_watch.rs:346`; tables `harness/*.rs` | | |
| 24 | Loop detection: Codex action rows (`• Ran`, `• Edited`, ...), Claude `● Tool(ARGS)`; harness retry banners (`Retrying in N seconds… (attempt k/n)`, Codex `■`/`⚠` rows) | C, Cx | `agent_watch.rs:629`, `:667` | | |
| 25 | Claude Code background work: footer counts (`2 shells · ← 1 agent`) and the closing row's `still running` (history once the input box shows) | C | `harness/claude.rs:254` | | |
| 26 | Claude Code scrolled up: `N new messages (ctrl+End) ↓` above the input box; the pane keeps no scrollback, so nothing below can be judged | C | `harness/claude.rs:303` | | draft 7 |
| 27 | Herdr can say `idle` while Claude Code's footer says `esc to interrupt` and output streams: the screen wins | H | `harness/claude.rs` `background_count`; `agent_watch.rs:398` | | draft 2 |
| 28 | Codex rate-limit modal (`Approaching rate limits`, `Switch to <model>` / `Keep current model`) recognised as a form | Cx | `harness_tui/codex.rs:26` | | |
| 29 | Muse forms are recognised but never answered by agentctl | M | `harness_tui/muse.rs:22` | | |

### Herdr glue and readiness

| # | Special case | Scope | Rust | Py | Upstream |
|---|---|---|---|---|---|
| 30 | Herdr reports an idle Codex as `unknown` (`fallback_reason: codex_state_ambiguous`); agentctl settles it from the screen | H, Cx | `HarnessAdapter::settle_herdr_state` `harness/codex.rs:141`; `client.rs:112`, `:2623`; `agent.rs:1873` | `client.py:1689`, `agent.py:1923` | draft 1 |
| 31 | Herdr's `agent start` times out on a fresh Codex sitting at `unknown`, and on a resumed Codex that starts working on its goal at once (never idle); agentctl proves readiness itself | H, Cx | `client.rs:1198`, `:1244` | `client.py:1070` | draft 5 |
| 32 | Working follow-ups: Codex always (it steers or queues), Muse only through its verified editor, Claude Code only with `deliver_while_working` | X (per-harness policy) | `HarnessAdapter::followup_policy`; `agent.rs:1837` | `agent.py:1913` | |
| 33 | Chat bridge types into a working pane only for Claude Code with `--deliver-while-working` | C | `chat_service.rs:4209` | `chat.py` | |
| 34 | Muse runs in a custom pane (`herdr-pane` adapter), pinned by its own process identity, since Herdr launches it as an arbitrary program | M, H | `subagents.rs:849`, `:2145`, `:7410`; `subagents/nested.rs:237` | `subagents.py` | |
| 35 | Root slot relay drives only Claude Code and Codex (Herdr has screen rules and agentctl has composer models for both) | C, Cx | `subagents.rs:4540`, `:4543` | | |
| 36 | Harness kind detection from a tmux pane's foreground command (`codex`, `claude`, `muse`, `.real` suffix) | H | `term-backend/src/tmux.rs:163`; `term.rs:814` | | |
| 37 | Claude Code session state from `claude agents --json` (idle/busy), keyed by `HERDR_PANE_ID` in each session's environment | C | `inbox/watch.rs:631`, `:701`; `chat_service/reply_reminders.rs:320` | | |

### Launch, sessions, resume, transcripts

| # | Special case | Scope | Rust | Py | Upstream |
|---|---|---|---|---|---|
| 38 | Structured presets: Claude `--resume ID --model M`, `--effort E`; Codex `resume ID --no-alt-screen --model M`, `--config model_reasoning_effort=E`; Muse `--model M`, `--reasoning-effort E`, `resume ID` after every option | C, Cx, M | `HarnessAdapter::launch_arguments`, `reasoning_arguments` in `harness/*.rs` (`harness/codex.rs:70`, `harness/muse.rs:64`); called from `subagents.rs` `harness_arguments`, `profiles.rs` `reasoning_arguments` | `profiles.py:685`, `launch_spec.py:168` | |
| 39 | agentctl assigns Claude Code's conversation id itself (`--session-id UUID`) so the record knows it before launch; raw Claude conversation selectors are refused | C | `harness/claude.rs:82`, `:158`; `subagents.rs:6976` | `revive.py:199` | |
| 40 | Replaying a recorded launch: each harness's selectors (`--resume`/`-r`/`--session-id`/`-c`/`--continue`/`--fork-session`; Codex leading `resume ID`, `--last` refused; Muse trailing `resume ID`) replaced by one explicit resume | C, Cx, M | `HarnessAdapter::split_conversation`, `replay_arguments`; `harness/claude.rs:201`, `harness/codex.rs:84`, `harness/muse.rs:73` | `launch_spec.py:168`-`229` | |
| 41 | Revive's policy arguments (when no launch spec): the same selectors by harness | C, Cx, M | `subagents/revive.rs:571`, `:646` (not yet moved) | `revive.py:199`-`218` | |
| 42 | Profile validation: Codex `-c/--config` keys and `-p/--profile` conflict with structured model/effort; duplicate `resume`; Muse headless | Cx, M | `profiles.rs:250`, `:346`, `:580` (not yet moved) | `profiles.py:207`-`269`, `:438` | |
| 43 | Codex goals: `/goal TEXT` through the composer, native goal state through `codex app-server proxy`; refused behind a relay (slash commands cannot be relayed) | Cx | `subagents.rs:9595`, `:9677`; `codex_goal.rs` | `subagents.py` | |
| 44 | Worker reset: Muse gets a fresh session id, others none | M | `worker_reset.rs:206` | | |
| 45 | Skill directories: `~/.codex`, `~/.claude`; Muse through its own installer | C, Cx, M | `HarnessAdapter::skill_directory`; `skill_install.rs` | `skill_install.py:268` | |
| 46 | Transcript: Claude `$CLAUDE_CONFIG_DIR/projects/<cwd, non-alphanumerics → '-'>/<session>.jsonl` (default `~/.claude/projects`); not held open, so the session id is required | C | `harness/claude.rs:113` (`transcript_path`); `inbox/watch.rs` `transcript_of_session` | | |
| 47 | Transcript: Codex `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<thread>.jsonl`, held open per thread (sub-agent threads too); main thread is the one whose `session_meta` names no parent | Cx | `harness/codex.rs:153` | | |
| 48 | Transcript: Muse `$XDG_DATA_HOME/muse/sessions/YYYY/MM/DD/<session>/session.jsonl`; the runtime child holds `<session>/.session.lock` open (shown `(deleted)`), which names the session even though Muse reports none | M | `harness/muse.rs:96` | | draft 8 |
| 49 | agentcloud: journal input, no turn to interrupt, no local screen or transcript; skipped by the watcher; drivers `native`, `claude-code`, `codex`, `muse-code` | A | `harness/agentcloud.rs`; `subagents/cloud.rs:12`; `subagents.rs:8883` | | |

### Interrupts and urgent delivery

| # | Special case | Scope | Rust | Py | Upstream |
|---|---|---|---|---|---|
| 50 | Interrupt is Escape; refused on an approval prompt (Escape would deny it) or an operator draft (Enter would merge it); after Escape a Codex moves its queued inputs into the composer, where Enter submits them | X, Cx | `subagents.rs:8883`, `:8910` | | draft 6 |
| 51 | `send --urgent` waits for a tool boundary, then interrupts and submits; Muse steers at once | X | `subagents.rs:9009` | | |
| 52 | Misroute note typed only into Claude Code, Codex and Muse | X | `subagents.rs:3217` | | |
| 53 | Claude Code shell mode: a prompt starting with `!` is redrawn as `! text` and never verified, then quarantined (#559 claude-shell-mode-prompt, open) | C | not handled | | by design upstream; agentctl bug |

## Transcript facts used by the watcher

What each reader treats as model output, a request in flight, and a tool call's limit
(`HarnessAdapter::read_transcript`):

| | Claude Code | Codex | Muse |
|---|---|---|---|
| Turn start | `user` entry that is a prompt | `event_msg/task_started` | `runtime.user_intent.accepted`, `top_level_turn_started` |
| Model request | `api-request` entry (written when the request is sent) | not written: inferred at turn start and after the last tool result | `provider_request_options_configured`; also inferred after the last tool result |
| Model output | `assistant` entries (text, thinking, tool_use) | `reasoning`, assistant `message`, `agent_message`, tool calls | `model_response_created`, reasoning deltas and commits, `assistant_message_committed`, `assistant_tool_calls_committed`, `model_completed` |
| Tool call / result | `tool_use` / `tool_result` by id | `function_call`, `custom_tool_call` / `*_output` by `call_id` | `tool_batch.effect.started` / `.terminal` by `call_id`; arguments from the committed tool calls |
| Limit | Bash `timeout` (ms); Bash without one: the harness default, 120 s (`BASH_DEFAULT_TIMEOUT_MS` can change it); `run_in_background`; other tools none | `timeout_ms` or `yield_time_ms` (also read from a code-mode `exec` cell); `exec_command` without either: unified exec's default yield, assumed 10 s | `timeout_ms` or `yield_time_ms`; a task linked as a background stream; otherwise none |
| Bookkeeping (moves only the last-event clock) | attachments, titles, modes, queue operations, snapshots | `token_count`, `turn_context`, `world_state`, ... | hooks, reminders, resource samples, ... |

Only the last 4 MiB of a transcript is read (Muse journals pass 100 MB a day). Assumptions not
verified against harness source: Codex's 10 s default yield, and that a Claude `api-request` row is
written before the request rather than after its reply (the rows on this host are consistent
with that).

## Upstream drafts

Each is written to be filed as is. None has been filed.

### 1. Herdr: an idle Codex pane reports `unknown`, never `idle`

> Herdr's bundled Codex screen rules match only working and blocked screens. An idle Codex
> composer (`» Ask Codex to do anything` with the `? for shortcuts` footer, no timer row) matches
> no rule, so `herdr agent explain` reports `state: unknown` with `fallback_reason:
> codex_state_ambiguous`, and `herdr agent start --timeout` never sees the pane become idle.
> **Steps:** start Codex 0.159.1 in a pane with `herdr agent start NAME --kind codex`; wait for
> the composer; run `herdr pane get` and `herdr agent explain`. **Expected:** `idle`.
> **Actual:** `unknown`, permanently. **Suggested fix:** a rule for the idle composer and footer.

Evidence: `herdr_093_*` tests in `submission.rs` and `client.rs`; agentctl carries a 22-phrase
refusal list to settle it safely.

### 2. Herdr: Claude Code reported `idle` while a turn runs

> With Claude Code 2.1.x in a pane, `herdr pane get` sometimes reports `agent_status: idle`
> while the footer under the input box says `esc to interrupt` and output keeps streaming.
> **Expected:** `working` while the footer shows `esc to interrupt`. Seen on live panes on
> 2026-10-20.

### 3. Herdr: ready before the composer is drawn

> `herdr agent start` (and the pane's status) can report a freshly launched harness ready before
> it has drawn its composer, so input typed at once can land before the editor exists.
> **Expected:** ready only once the harness's input box is on screen.

### 4. Claude Code: a submission key dropped while the composer redraws

> Text pasted into Claude Code's input box and followed by Enter sometimes stays in the box: the
> Enter is dropped while the TUI redraws or runs a hook. A second Enter submits it. **Expected:**
> an Enter that arrives during a redraw is applied, not lost.

Status: described in the source comments of the verified-submission path and reproduced only
with a fake screen here; agentctl retries the key with doubling waits.

### 5. Herdr: `agent start --timeout` cannot succeed for a resumed Codex with an active goal

> `herdr agent start NAME --kind codex -- resume ID` waits for an idle prompt. A Codex resuming a
> thread with an active goal starts working at once and is never idle, so the start times out
> although the harness is running and correct. **Expected:** a start option that accepts a
> working harness (or one that reports the native session it resumed).

### 6. Codex: input queued with Tab waits for the whole turn, however long

> While a turn runs, Tab queues a follow-up under `Queued follow-up inputs`, and Codex submits it
> only when the turn ends. An autonomous turn that keeps calling tools can run for days: one ran
> 46 h 54 m while every message queued to it waited, invisible to the model. Enter instead steers
> at the next tool call. **Request:** show each queued item's age; let a queued item be promoted
> to a steer (or drain queued items at tool boundaries after a configurable age); and expose the
> queue in the rollout so supervisors can see it without screen scraping.

Status: this is Codex's documented Tab behaviour, not a malfunction. agentctl chooses Tab for a
busy Codex (inventory item 5) so that it never steers a turn by accident, which is what put messages in
that queue. The watcher now reports long turns with queued input and, under the auto-interrupt
policy, interrupts only a stuck one.

### 7. Claude Code: a scrolled-up view hides everything below it

> When the conversation view is scrolled up, Claude Code freezes it and keeps no terminal
> scrollback, so a supervisor reading the pane cannot see the running turn, its newest output, or
> its final reply until someone scrolls down. **Request:** a status line that stays visible while
> scrolled (the running-turn row), or a read-only status command.

### 8. Muse: no native session id for supervisors

> Herdr reports no `agent_session` for a Muse pane (every Muse record on this host had none on
> 2026-10-27), and Muse prints its session id nowhere on screen, so a supervisor cannot record
> which conversation an agent runs, resume it, or find its journal except through the
> `.session.lock` its runtime holds open. **Request:** report the session id to the terminal host
> (Herdr's agent-session hook) or print it at startup.

## Not moved yet

The Python edition keeps its copies (the differential tests compare the editions). In the Rust
edition, revive's policy arguments (item 41), profile validation (42), Codex goals (43), worker reset
(44), the chat bridge's Claude-only rule (33), Claude session discovery for the inbox watcher
and reply reminders (37), and the Muse editor (3) still branch on the harness name in place;
each is listed above with its location so it can move behind the adapter in its own change.
