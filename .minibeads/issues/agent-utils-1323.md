---
title: 'muse-drawn-prompt-missed: a prompt fully drawn in the Muse editor is reported as not drawn, and Enter is never pressed'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-11T17:00:50+00:00
updated_at: 2026-11-11T17:00:50+00:00
---

# Description

[opus 5.5] A regression of #1085 muse-send-staged-unsubmitted (closed). On 2026-11-11 at about 11:55 AM ET the coordinator sent one `agentctl send NAME --queue-only --file F` notice (about 700 bytes, 7 lines) to each of ten Muse lanes. For two of them, one busy and one idle, the agent watcher then reported:

    messages to mu-edone are held: 1 waiting, the oldest for 13 min, and every attempt types nothing. Why: stranded_prompt: input_stalled: late_paste: pane w16:p9J: Muse has not drawn the pasted prompt within 60s (its editor is still empty), so it is not taking terminal input; no key was sent and the editor was left untouched. NOT delivered yet: the prompt is never typed again, Enter is pressed only once the whole prompt shows in the editor, and nothing else is typed into this pane until then.

and the same for mu-ignsalv (pane w16:p9E).

At the same time, `agentctl read` showed the whole prompt drawn in each Muse editor, unsubmitted, below the editor's top rule:

    ❯ [from coordinator via agentctl · reply: agentctl send coordinator]
      coordinator -> all lanes on this host: the data disk is low and another team's builds are frozen. Until further notice:
      (1) Run at most ONE local validate or build at a time per lane, and never in parallel nested worktrees.
      (2) As soon as a validate or gate finishes (never mid-validate), delete the regenerable build output it left in your slot or nested worktrees: cargo target/
      dirs (rs/target, vibe-talk/target, etc.) and page build trees. If your next validate would rebuild them anyway, you may keep the ones in your MAIN slot
      checkout, but delete extra nested-worktree copies.
      No reply needed; just do it at your next safe point.
    ────────────────────────────────────────────────
      watermelon-full-preview · xhigh · <slot path> · Launch overrides

So the "is the pasted prompt drawn" check misses a prompt that is on screen: the editor is not empty, and the prompt is never submitted. Note that the drawn prompt wraps long lines and is prefixed by the editor's `❯` and two-space continuation indent.

Builds and versions:
- Watcher (`agentctl watch-agents`, which drains queued items): services snapshot ba289c70cb78 (agent-utils main of 2026-11-11 03:16 PST).
- Sender: the tracked Cargo launcher `agent-utils/bin/agentctl` (rust cache source 3bfda4df5008), harness checkout a5296c58.
- Muse Code 1.4.17 (1.4.17-R8362.1), model watermelon-full-preview, reasoning xhigh.
- No agentctl commit after a5296c58 touches the drawn-prompt check (per the lead: only 631a17cd slash-command staging and ae8f2d96 revive).

# Design

- Reproduce with a fake Muse pane whose screen shows a wrapped, multi-line pasted prompt with the `❯` prefix and continuation indent, and find why the drawn check reports the editor as empty.
- The check should compare the prompt against the editor text after undoing wrapping and the editor's prefix and indent, in the same way for idle and busy Muse.
- When the prompt is fully drawn, press Enter once (as #1085 intended); when it isn't, keep today's refusal.

# Acceptance Criteria

- A regression test with the screen above (wrapped lines, `❯` prefix, indented continuation, status bar below) detects the prompt as drawn and submits it exactly once.
- A `--queue-only` notice to an idle and to a busy Muse lane is submitted when that lane can take input, with no stranded_prompt notice.
- An editor that really is empty still gets the late_paste refusal.

# Notes

The quoted screen has the notice's host, team and path names redacted; so its first line is shorter than captured; the wrapping of the other lines is as captured. Both affected panes were left untouched. Each lane will see the notice whenever it next submits.

## Resolution

[watermelon-full-preview] Cause: not the wrapping. The Muse drawn-prompt and staged-draft checks already read the editor region only (between the last two rules, above one status footer) and undo the wrapping, the `❯` prefix and the continuation indent; a fixture built from the screen above passes them, in both the Rust and Python editions. What went stale was the screen itself. `agentctl watch-agents` observes through a Herdr client that keeps each read-only answer for the whole pass (`HerdrClient::snapshotting`, #571 watch-agents-scale), and its queue drain types through that same client. A delivery's first pane read in a pass was therefore the only one Herdr was ever asked: after the drain pasted the notice, every poll for the drawn prompt was answered with the pre-paste screen (editor still empty), so the notice was stranded as `late_paste` and never submitted in that pass. Likewise, the stale-draft recovery pressed Enter on a drawn draft and then kept reading the pre-Enter screen, so a draft the lane had in fact submitted (its copy now in the transcript above an empty editor) read as still staged after 3 Enter key presses. A direct `agentctl send` uses a non-snapshotting client and was never affected, which is why only watcher-drained lanes stranded.

Fix: any non-read invocation through a snapshotting client (typing text, pressing a key, reporting a status) now drops the pass's kept answers, so the next read asks Herdr again. Observation passes type nothing, so their caching is unchanged. The drawn/staged checks themselves are unchanged in behaviour and are now pinned by fixtures.

Regression tests: a snapshotting-client test whose pane screen changes on Enter (the post-Enter read asks Herdr again; it returned the kept pre-Enter screen before the fix); a fake Muse editor that wraps staged text at 80 columns, through which the notice above is delivered idle and busy with exactly one paste and one Enter; editor-level fixtures (Rust and Python) for the wrapped drawn prompt (drawn whole, and vouched as a draft by its editor text), a really empty editor (no draw, so the late_paste refusal stands), and a submitted copy in the transcript above an empty editor (no draft, pane reads idle, not staged).

Noted separately, not fixed here: one lane in the same minute got `cannot invoke Herdr: control command was cancelled`, a pass-deadline cancellation of a Herdr call, which types nothing and retries on a later pass.
