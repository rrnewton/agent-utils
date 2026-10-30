# Coding-agent harness facts: transcripts

A coding agent running in a terminal shows its work on screen, and it also appends the same
conversation, exactly as the model wrote it, to a session file of its own. The screen is a
rendering: Markdown markers become styles, long lines wrap at the terminal's width, tables become
box drawing, and text written in the middle of a turn can collapse to one line. Tools that drive
such agents through their terminals, and tools that archive what they did, need the original text.

This library finds the session file of a running agent and reads it as it grows, for three
harnesses:

| Harness | Session file | Found by |
|---|---|---|
| Claude Code | `~/.claude/projects/<project>/<session>.jsonl` (or under `$CLAUDE_CONFIG_DIR`) | the session the running process names in `sessions/<pid>.json`, or a session id |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (or under `$CODEX_HOME`) | the rollout the process holds open, or a thread id |
| Muse | `~/.local/share/muse/sessions/YYYY/MM/DD/<session>/session.jsonl` | the session lock the runtime holds open, or a session id |

## Use

```rust
use agent_harness::{transcript, Event, Follower, Hints};

let reader = transcript("claude").expect("a known harness");
let hints = Hints { pid: Some(4242), ..Hints::default() };
if let Some(found) = reader.locate(&hints) {
    println!("reading {} ({})", found.path.display(), found.evidence);
    let mut follower = Follower::open(&found.path, reader.decoder(), 4 << 20);
    for event in follower.poll().expect("readable") {
        if let Event::Text { text, .. } = event {
            println!("{text}");
        }
    }
}
```

- `Transcript` is one harness's knowledge: `locate` turns `Hints` (the harness process, its
  working directory, a session id) into a file and the evidence that chose it, and `decoder`
  turns lines into `Event`s: prompts (typed, or queued into a running turn), the model's text,
  tool calls, and turn ends. Reasoning, tool output and bookkeeping are left out.
- `Follower` reads complete lines only, from where it stopped. Its first read starts a chosen
  number of bytes before the end, and it starts over when the file is replaced or truncated.
- `replies::complete_blocks` finds the reply blocks an agent writes between a line
  `<CHAT_REPLY_ID>` and a line `</CHAT_REPLY_ID>`, and `replies::rendered_equivalent` tells
  whether a block read from a terminal is a rendering of a block read from a transcript (same
  letters and digits, whatever the styles, wrapping, borders or table layout).

Everything here is a read-only fact about a harness. The library never writes to a session file
and never talks to a harness.

## Dependencies

`serde_json`. Process facts come from Linux's `/proc`; on other systems only a session id given
in `Hints` locates a file.

## License

MIT.
