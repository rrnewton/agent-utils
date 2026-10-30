# chat-prompt-anatomy: what a bridge types, and how an agent finds its bridge

Date: 2026-10-30. Issue: #707 chat-prompt-anatomy. Status: pieces 1 and 2 landed (e4a03f3e,
f2f4ff15); piece 3's code landed after them, and moving each live bridge is an operator step
that waits for the owner's go-ahead.

## The problem

A request prompt the chat bridge (`agentctl chat run`, Rust edition) types today looks like this,
once it is unwrapped:

```text
Sent 2026.10.30:10:51 EDT. The user's request arrived through the configured chat bridge.
Source: C0EXAMPLE/messages/1761835860.123456
Sender: U0EXAMPLE
Thread: C0EXAMPLE/threads/1761835000.000100 (a reply in an existing thread)
To read earlier messages in this thread, run: /home/me/.../snapshots/845b960e615e/agentctl chat thread --bridge-state /home/me/.local/state/agentctl-slack-x/bridge --thread C0EXAMPLE/threads/1761835000.000100 --last 10
<the message>

Complete this request using your normal instructions and tools. You may send one or multiple
replies ... Include the line <CHAT_REPLY_5380> at the beginning ... (a paragraph)

Reply between the two lines as usual ... run this command ...: /home/me/.../agentctl chat reply
--bridge-state /home/me/.local/state/... --request <64 hex> --reply-id 5380 --file
PATH_TO_YOUR_REPLY
Run it once for each reply; ... (another paragraph)
```

Four metadata lines, two long absolute paths, a 64-character key and about 250 words of rules,
for every message. The owner's own words are hard to find in it, and the rules are the same in
every prompt.

## Piece 1: the prompt's anatomy

One header line in the grammar of `agentctl send`'s sender header (#609
sender-header-mandatory), a blank line, the message between two tag lines, a blank line, and a
short footer:

```text
[from Ada Lovelace via slack #team-chat · sent: 2026.10.30:10:51 EDT · reply: <CHAT_REPLY_5380> or agentctl chat reply 5380]

<chat_message>
the message, exactly as sent
</chat_message>

Reply between a line <CHAT_REPLY_5380> and a line </CHAT_REPLY_5380>, every reply in its own pair, with no tool call between them. Earlier messages in this thread: agentctl chat thread 5380
If you have trouble, run: agentctl chat help 5380
```

- **Header.** `[from SENDER via ROUTE · KEY: VALUE ...]`, parsed by `sender_header::parse`.
  SENDER is the sender's label from the bridge config's new optional `labels.senders` map
  (provider ID to display name), else the provider ID. ROUTE is the subscription plugin
  (`slack`) and the channel's label from the new optional `channel_labels` map (`#team-chat`),
  else the channel ID. `sent:` carries what the old `Sent ...` opening carried, including
  `delivered 1 h 12 min later` for a late prompt. `reply:` names the reply markers, and the
  command when the bridge offers it.
- **Labels and the grammar.** A display name has spaces (`Ada Lovelace`), which the header
  grammar's SENDER did not allow. The grammar is widened, in both editions' parsers: SENDER may
  hold single spaces between words, still no tab, line break, `·` or `]`, and no ` via `.
  `agentctl send` never writes such a sender, so nothing it parses changes. A label that breaks
  these rules is refused when the configuration is read; a label is printed as configured.
- **Fence.** `<chat_message>` and `</chat_message>` on lines of their own: unambiguous for a
  model, a few tokens, and they say exactly where the sender's words start and stop. A message
  line that reads as a tag (any case, invisible characters ignored) is printed with `<\` in
  place of its first `<`, as `<\/chat_message>`, so the fence cannot be closed early. The tags are stable; tools may rely on them.
- **Footer.** The marker rule in one sentence, the thread command for a reply in an existing
  thread, and one help line. The long rules (when to use the reply command, sending each reply
  one way only, what to do when the command fails, checking `chat sent`) move to
  `agentctl chat help [REPLY_ID]` and the chat userguide, and the `agentctl-chat` skill.
- **Variants.** A bridge whose screen is never read (headless coordinator) gives the command
  only: `reply: agentctl chat reply 5380 --file FILE` and "Nothing you print reaches the chat;
  write each reply to a file and run: agentctl chat reply 5380 --file FILE". An inbound-only
  bridge gives no reply field and says outbound chat is off. A request without a short alias
  keeps its long `<nonce>_<ordinal>` ID and the "add one to the last number" rule.
- **Dropped.** `Source:` and the message's thread ID are not printed; `agentctl chat help 5380`
  and `agentctl chat inspect` show them.
- **Wrap safety.** Every marker the prompt prints is followed by text on its row (` and`, `,`,
  `]`), so no wrapped row is a marker line; the existing any-width test covers the new text.

### Retiming a queued prompt

The queue retimes a prompt when it types it (`prompt_time::retime`): today it replaces the
recorded `Sent ...` opening at the start of the text. The time now sits inside the header, so
the queue records the `sent: ...` field as the opening and `retime` replaces that field in the
first line. A queued item recorded by an older build (opening `Sent ...`) is retimed as before.
An older drain that meets a new item fails to find the opening at the start and types the text
as stored, which is that function's documented fallback: the time is right, only the delay is as
of queueing. The Python edition's `prompt_time.retime` mirrors the change, since either
edition's drain can type a queued prompt.

## Piece 2: commands that find their bridge

`agentctl chat reply 5380 --file F`, `agentctl chat thread 5380`, `agentctl chat sent [5380]`
and `agentctl chat help [5380]` take a positional reply ID and no `--bridge-state` or
`--request`. agentctl finds the bridge the way `git` finds its repository:

1. `--bridge-state DIR` when given (old prompts, scripts), or `--bridge NAME`.
2. `AGENTCTL_CHAT_BRIDGE` when set (a state directory; for a launcher that wants to pin it).
3. Otherwise the candidates are every bridge registered for discovery (below). Each bridge's
   `bridge.json` names its target agent (`agent_name`, kept current by `chat repoint`). The caller
   is the registered agent whose pinned harness is the nearest ancestor of the command (the
   registry's `caller_edge`, as `agentctl send` uses for its sender header), else
   `AGENTCTL_AGENT`, which counts only in the working registry and only when no registry knows
   the caller. When the caller is known, candidates whose target is not the caller are dropped
   (none left is an error); only when it is known nowhere are all candidates kept. Of those,
   when any is live (a `chat run` holds its runner lock, read from `/proc/locks`), only the live
   ones are kept, so a reply never goes to a stopped bridge whose state was left behind.
4. A reply ID narrows further to the bridge that gave it: its reply alias record names the alias
   (bridges typing into one pane already have disjoint `reply_alias_range`s), or one of its
   requests has the long ID's nonce. With no caller identity the reply ID alone must pick one.
5. Exactly one bridge is used; none or several is an error that lists what was tried and says
   to pass `--bridge NAME` or `--bridge-state DIR`.

`--request KEY` still works with `--bridge-state`; `--reply-id ID` is the same as the positional
ID; `--thread T` still works. Every command an older prompt printed keeps working unchanged.

**Discovery.** A bridge is found in two places: `REGISTRY/bridges/<name>/` (piece 3), where the
registry is `--registry` or, when that is the default `.agentctl` and it does not exist in the
working directory, the nearest `.agentctl` above it; and the per-user index
`$XDG_STATE_HOME/agentctl/chat-bridges/<id>.json` (default `~/.local/state`), one file per bridge
state directory, which `chat run` writes when it starts: the state directory, the registry, and
the time. The index only says where to look; the state's own `bridge.json` and the registry
decide. An entry whose state directory is gone is skipped.

**Bare `agentctl`.** The prompt prints `agentctl` (not the service's absolute path) while the
`agentctl` on the service's `PATH` is this executable or prints the same `--version` with a
commit (checked at most a minute before each prompt, so a rollback of the `PATH` program is
followed), so an older build on `PATH` never receives syntax it does not know. It prints the short
form (reply ID only) when the bridge registered itself in the index; otherwise it prints the old
explicit form. `chat help 5380` always ends with the explicit absolute command as a last resort.

## Piece 3: bridge state lives with the project (code landed; each move needs the owner's OK)

- New home: `REGISTRY/bridges/<name>/`, the whole state directory, one subdirectory per bridge
  (the rule for growing collections). `bridges` joins the registry's reserved names (`archive`,
  `agents`, `locks`) in both editions' readers already in piece 2, so no agent can take it.
- Every chat command must accept a `--bridge-state` that is a link (canonicalized when the link
  is the user's own), since today a state directory that is a symlink is refused as unsafe.
- `agentctl chat init --bridge NAME` creates state there; every chat command accepts
  `--bridge NAME` for `--registry`'s `bridges/NAME`.
- `agentctl chat migrate-state --bridge-state OLD --name NAME [--apply]`, a dry run by default:
  refuses while a `chat run` holds the state (runner lock) or a channel lock names it; moves the
  directory with one `rename` (same filesystem; a move across filesystems is refused, with the
  copy steps in the message), and leaves a symlink at OLD pointing at the new place, so systemd
  units, `services` snapshots, older prompts' commands and `--bridge-state OLD` keep working
  without edits. The service is then started again.
- Deployment order: every agentctl that reads the registry (watchers, the team `bin`, a shared
  `~/.local/bin/agentctl`) must be upgraded first. Checked 2026-10-30 with the deployed build of
  that day: `agentctl list` shows `bridges` as an agent with `record_error: true`, and `status
  bridges` fails, so a watcher on such a build would report it; the new build lists nothing.
  Older builds cannot be taught otherwise (a name they would skip, such as `.bridges`, would hide
  the directory from people), so the upgrade comes first, as with #593.
- Old locations are not deprecated: `--bridge-state DIR` remains a general way to name any state
  directory, so no removal issue is needed.

## Not in scope

- Display names fetched from the provider (a `resolve_sender` in reverse). Labels are config.
- The Python edition's bridge, which has its own prompt and state format (`chat context
  --state`); only its `prompt_time.retime` mirrors this change.
- Reply reminders keep their wording; they print the same short commands once those are on.
