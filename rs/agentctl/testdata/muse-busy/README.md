# Muse 1.4.14 screens during a running turn

Visible-screen captures (`herdr pane read --source visible`) from throwaway Muse 1.4.14 sessions
started on 2026-10-25 for #505 agentctl-muse-followup, launched with `--trust-workspace
--approval-mode never` (footer `Launch overrides`), or `--approval-mode untrusted
--approval-judge off` for the approval screen. Only the launcher banner above the versioned
header (or, where Muse had scrolled the header away, everything above the last 24 rows) was
dropped, trailing blanks were trimmed, and the session directory was replaced by
`~/work/project` or `/work/project`; no other text changed.

| File | What the session was doing |
|---|---|
| `muse-idle.txt` | Turn finished; empty editor. |
| `muse-working-thinking.txt` | Turn just started: `◆ Thinking (8s · esc to interrupt)` above an empty editor. |
| `muse-working-command.txt` | Foreground command running; its output rows sit below the progress row. |
| `muse-working-draft.txt` | Text typed into the editor mid-turn, before any key. |
| `muse-steer-notice.txt` | 0.4 s after Enter: the prompt is a new user turn and the footer carries the notice `steering the running turn`. |
| `muse-steered.txt` | 3 s after Enter: the notice is gone (a `Hook ·` notice instead); a terminals panel sits between the editor and the footer. |
| `muse-queued-notice.txt` | 1 s after Tab: a `◆ Queued input · ↑ to edit` block and the footer notice `queued for the next turn`. |
| `muse-working-queued.txt` | The queued input still waiting while the turn runs. |
| `muse-steered-multiline.txt` | A three-line bracketed paste steered into the turn. |
| `muse-help-panel.txt` | `/help` typed mid-turn: the shortcuts panel replaces the footer (`tab to queue while running`). |
| `muse-approval.txt` | A command approval form in place of the editor, queued input above it. |
| `muse-double-checking-queued.txt` | After the approval: `◆ Double checking`, queued input still held. |
| `muse-interrupting.txt` | Esc with a draft staged: the turn is interrupted (`interrupting run`) and the queued input submitted. |
| `muse-wrapped-draft.txt` | A long paste staged mid-turn, wrapped inside `odd/places`. |
| `muse-overlong-token-draft.txt` | A 210-character token wrapped mid-word. |
