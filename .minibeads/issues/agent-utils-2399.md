---
title: 'agentctl-muse-delivery-unrecorded-paste: a Muse delivery stopped after its paste leaves the prompt pending with no attempt, and the next drain calls its own paste foreign'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-27T14:59:01-05:00
updated_at: 2026-11-27T17:10:39-05:00
closed_at: 2026-11-27T17:10:39-05:00
---

# Description

[opus 5.5] A consuming team's lead reported it twice on 2026-11-27, on
the same Muse lane. The lane's editor held a draft that was the whole
text of a prompt still waiting in its queue. The second time the draft
also showed agentctl's own sender header (`[from LANE via agentctl ·
reply: agentctl send LANE]`), so an agentctl delivery typed it. The queue
item still read `delivery_attempts: 0`, `delivery_state: pending`, with
no `stranded_at`, `possibly_submitted` or `typed_as`, and with the
error "the Muse editor holds a draft that agentctl did not type (a
pasted N characters); a person may be typing it, so agentctl neither
submits nor clears it ...; nothing was typed". So agentctl refused its
own paste as a foreign draft. Five later prompts waited 11+ minutes
behind it, while the lane sat in a 26-minute foreground wait for exactly
that reply. Both prompts were sent with `agentctl send --message-id`
from another lane, while the target was inside a long foreground
command. The workaround was Enter in the pane, then `agentctl cancel`
on the queued copy.

The evidence holds the two set-aside queue items, pane captures before
and after the Enter, and the agentctl versions involved: the sender's
release and the watch service's snapshot. Both predate main, and main's
delivery, queue and draft-attribution code is unchanged since them.
The other registry kept no per-agent event log for that agent, so which
process typed each paste is not on record.

How the state comes about (from the code, reproduced by a fake-pane
test):

1. The Muse typing paths never report that typing began. Every drainer
   that types should call `AgentRuntime::input_started` just before its
   first input effect. A queue drain uses that call to stamp
   `paste_started_at` on the inflight item (`PasteTrackingRuntime`).
   Both Muse paths in `rs/agentctl/src/subagents.rs` paste the prompt
   with `GuardedTerminal::send_text` and never make that call: the
   follow-up into a running turn (`muse_followup`), and the idle path
   (`run_with_runtime` for a Muse composer). The native-prompt branch
   of `run_with_runtime` doesn't make it either. The verified path
   (`submission.rs`), the term backend and the unknown-harness path all
   do.
2. A stop after the paste is filed as "nothing was typed". The send's
   deadline wraps the whole drain runtime (`UntilRuntime`,
   `send_deadline_seconds`, default 600 s). SIGTERM and SIGINT set the
   same flag. Once either fires, the next process check or screen read
   in the Muse delivery fails with an `AdapterErrorKind::Cancelled`
   error. `deliver_one` (`rs/agentctl/src/agent.rs`) maps a Cancelled
   error to `Delivered::NotStaged("delivery was cancelled before
   anything was typed")` when `typed_anything()` is `Some(false)`, and
   on these paths it always is. The NotStaged branch then returns the
   item to its inbox lane as pending. It records no attempt and drops
   `possibly_submitted` and `typed_as`. The paste stays in the editor.
   A send whose readiness wait ends near its deadline starts typing with
   seconds left, and a follow-up into a running Muse turn can take tens
   of seconds to prove, so the deadline often passes mid-delivery.
3. A drainer that dies after the paste ends the same way. Recovery
   (`repend_never_typed` in `rs/agent-queue/src/queue.rs`) treats a
   tracked inflight item without `paste_started_at` as provably never
   typed and returns it pending, with no attempt and no `stranded_at`.
4. The next drain refuses its own paste. It moves the item to
   `inflight/` and reaches `recover_muse_draft`.
   `stale_draft::find` only searches records in `processed/`, `failed/`
   and stranded inbox records. The prompt being delivered is in none of
   them, so its own text, or its `[Pasted Content N chars]`
   placeholder, is "a draft that agentctl did not type". The refusal is
   NotStaged, so the item goes back to pending, and every pass repeats
   this until a person presses Enter.

Two drainers racing on one item doesn't explain it. The queue lock and
the per-item claim lock serialise them, and an item claimed by a live
drain is never typed again.

# Design

- Record durably that typing began, before the first input effect, on
  every path that types for a drain. Add a fallible
  `AgentRuntime::record_input_started` (default: call `input_started`).
  The queue drain's tracking runtimes write `paste_started_at` through
  the durable JSON write and return its error, and the runtimes that
  wrap them forward it. The Muse follow-up path, the Muse idle path and
  the native-prompt branch call it just before the paste, after every
  pre-typing check. If the stamp can't be written, nothing is typed and
  the prompt stays pending (NotStaged).
- With the stamp in place, a stop after the paste is no longer
  NotStaged. A Cancelled error is filed like any unknown outcome:
  `failed/`, `possibly_submitted`, one attempt recorded. A drainer that
  dies is recovered as stranded (`stranded_at`), so its paste is flushed
  with Enter and never typed again. Either record lets the next delivery
  attribute the draft to agentctl.
- The prompt being delivered is recognised as agentctl's own. When the
  Muse editor holds that prompt, `recover_muse_draft` doesn't call it
  foreign. That covers older drainers, and a stamp lost to a filesystem
  error. The draft must be framed (a sender header, an `[agentctl ...]`
  notice or a batch) and be either:
  - its exact typed text, wrapping and the `sent:` field ignored; or
  - a `[Pasted Content N chars]` placeholder whose N is that prompt's
    paste length, when no other typed record in the placeholder lookback
    has that size.
  The delivery doesn't type the prompt again. It submits the copy
  already in the editor, with the same Enter-and-proof steps
  (`muse_followup_submit` while working, the idle path's Enter and
  confirmation otherwise). It records the evidence, and the prompt is
  delivered exactly once. Any other draft is still refused, never
  submitted or cleared.
- Rust only: the Python edition has no Muse follow-up path. If a support
  cell changes, the harness support matrix changes in the same commit.
- Only fields that already exist are persisted: `paste_started_at`,
  `stranded_at` and `possibly_submitted`. Readers pinned to older
  releases already read them.

# Acceptance Criteria

- A fake Muse pane test, busy and idle: a stop that lands during the
  first screen read after the paste. On main it leaves the item pending
  with `delivery_attempts` 0 and its paste in the editor, and the next
  delivery refuses that paste as "a draft that agentctl did not type".
  With the fix, the item is recorded as typed (`failed/`, possibly
  submitted, with `paste_started_at`). The next delivery submits the
  draft as agentctl's and delivers its own prompt. Each prompt reaches
  the transcript exactly once.
- A test proves `paste_started_at` is on the inflight record before the
  Muse paste reaches the pane, for both the follow-up and the idle path.
- A drainer that dies after a Muse paste is recovered as stranded, never
  as never-typed, and its paste is flushed with Enter, not typed again.
- An item left pending with its own paste in the editor, as older
  drainers leave it, is delivered by submitting that copy: by text and
  by a placeholder of unique size, busy and idle, with no second paste.
  Foreign text, a placeholder whose size another typed record shares,
  and an unframed draft are still refused untouched.
- No wall-clock budgets: poll counts and fake clocks only (owner rule
  346).
- `cargo fmt --check` passes for `rs/`, the focused agentctl tests pass,
  and the gate passes on the landed tree.

# Notes

Related: #720 agentctl-muse-stale-draft, #884
pane-lock-held-on-unobserved-paste (paste tracking and stranded
recovery), #1085 muse-send-staged-unsubmitted, #1732
agentctl-muse-midturn-typed-not-submitted, #1882
agentctl-false-failed-deliveries, #1995 agentctl-muse-working-delivery,
#2036 agentctl-send-timeout-inotify-wait (the deadline runtime).

Follow-up observation, not fixed here: `agentctl send --deadline`
promises that "a prompt being typed is finished first", but the deadline
runtime also cancels a delivery that has already typed, on every
harness. This fix makes that cut recorded and recoverable. It doesn't
stop the cut.

## Resolution

[Claude Opus 5.5] Landed on main as 14d09442 (full gate PASS, 54/54
steps, on 4d617093). The Muse follow-up path, the Muse idle path and
the native-prompt branch now call the new fallible
`AgentRuntime::record_input_started` just before the paste, after every
pre-typing check. The queue drain's tracking runtimes write
`paste_started_at` through the durable JSON write and return its error.
If the stamp can't be written, nothing is typed and the prompt stays
pending. A stop after the paste is now filed as typed (`failed/`,
`possibly_submitted`, one attempt), and a drainer that dies is
recovered as stranded and flushed with Enter. `recover_muse_draft`
also recognises the prompt being delivered
(`stale_draft::prompt_being_delivered`): a framed draft that is its
exact text, or a `[Pasted Content N chars]` placeholder whose size no
other typed record in the lookback shares. The delivery then submits
the copy already in the editor instead of typing again, busy or idle,
and a readiness wait that finds that draft staged goes on to deliver.
A placeholder whose size another record shares is refused, and the
refusal names both candidates. Foreign and unframed drafts are still
refused untouched. Tests in
`rs/agentctl/src/subagents/tests/unrecorded_paste.rs` (5) and one
`stale_draft` unit test cover these cases. All 5 fail on the parent
with the incident's wording ("a draft that agentctl did not type",
`delivery_attempts` 0); the parent also typed the prompt again under a
shared-size placeholder. The harness support matrix's Muse stuck-paste
cell says so. No new persisted fields. The deadline follow-up above is
still open.
