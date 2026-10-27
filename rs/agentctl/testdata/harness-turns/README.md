# Harness screens around a long running turn

Fixtures for #570 agent-progress-monitoring: what a harness draws while a turn runs with input
queued behind it, and after that turn is interrupted.

The Codex files come from visible-screen captures (`herdr pane read --source visible`) of two
Codex lanes on 2026-10-26, one of which had stayed inside a single turn for 46 h 54 m while every
message sent to it waited in its `Queued follow-up inputs` block. Their structure is kept exactly:
the glyphs (`•`, `↳`, `■`, `›`, `»`, `└`, `│`), indentation, wrapping, the `…` that elides a long
queued item, the timer rows, the edit hint, the composer and the footer. The text of tool output,
queued messages, paths, model labels and session titles was rewritten to remove every project,
host and agent name; nothing else changed.

| File | Source | What it shows |
|---|---|---|
| `codex-working-queued.txt` | captured | A turn at `Working (2h 40m 18s • esc to interrupt)` with two queued inputs, both wrapped and elided. |
| `codex-long-turn-queued.txt` | captured tail, queue rebuilt | The 46 h 54 m turn before the interrupt: the timer row the lane showed, then a queue of four inputs in the captured block's layout (the capture was taken after the interrupt, so the queue's rows are rebuilt from the other lane's block). |
| `codex-interrupted-composer.txt` | reconstructed | Just after Escape: `■ Conversation interrupted`, and the queued inputs moved into the composer, where Enter submits them. |
| `codex-after-interrupt.txt` | captured | After Enter: the merged queued inputs as one user turn, the lane's reply, and a new `Working (10m 30s • esc to interrupt)` turn with nothing queued. |
| `claude-working-queued.txt` | constructed | Claude Code mid-turn with its spinner row (`✻ Sautéing… (12m 4s · ↓ 8.4k tokens · esc to interrupt)`) and the `Press up to edit queued messages` hint it draws in an empty input box while messages wait. |

Muse's queued-input screens are in `../muse-busy/`.
