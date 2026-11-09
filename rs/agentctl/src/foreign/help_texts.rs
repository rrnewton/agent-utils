//! Verbatim help texts for the foreign-worker dispatcher.

/// Verbatim `herdr-subagents --help` help.
pub const TOP_HELP: &str = r##"usage: herdr-subagents [-h]
                       {up,send,read,status,down,inbox,reset,migrate,backend,turn,keeper,mcp}

Manage persistent foreign-harness workers.

positional arguments:
  {up,send,read,status,down,inbox,reset,migrate,backend,turn,keeper,mcp}

options:
  -h, --help            show this help message and exit
"##;

/// Usage line(s) of `herdr-subagents --help`.
pub const TOP_USAGE: &str = r##"usage: herdr-subagents [-h]
                       {up,send,read,status,down,inbox,reset,migrate,backend,turn,keeper,mcp}"##;

/// Verbatim `herdr-subagents backend --help` help.
pub const BACKEND_HELP: &str = r##"usage: herdr-subagents backend [-h] [--set {tmux,herdr}]

Show or set the subagent presentation backend.

options:
  -h, --help          show this help message and exit
  --set {tmux,herdr}
"##;

/// Usage line(s) of `herdr-subagents backend --help`.
pub const BACKEND_USAGE: &str = r##"usage: herdr-subagents backend [-h] [--set {tmux,herdr}]"##;

/// Verbatim `herdr-subagents send --help` help.
pub const SEND_HELP: &str = r##"usage: herdr-subagents send [-h] [--message-file MESSAGE_FILE] [--model MODEL]
                            name [message]

Send a message to a subagent.

positional arguments:
  name
  message               message text

options:
  -h, --help            show this help message and exit
  --message-file MESSAGE_FILE
                        file holding the message
  --model MODEL         per-turn model override
"##;

/// Usage line(s) of `herdr-subagents send --help`.
pub const SEND_USAGE: &str = r##"usage: herdr-subagents send [-h] [--message-file MESSAGE_FILE] [--model MODEL]
                            name [message]"##;

/// Verbatim `herdr-subagents read --help` help.
pub const READ_HELP: &str = r##"usage: herdr-subagents read [-h]
                            [--last | --since-turn SINCE_TURN | --tail TAIL]
                            name

Read a subagent's transcript.

positional arguments:
  name

options:
  -h, --help            show this help message and exit
  --last                last turn's final answer
  --since-turn SINCE_TURN
                        from turn N onward
  --tail TAIL           last K transcript lines
"##;

/// Usage line(s) of `herdr-subagents read --help`.
pub const READ_USAGE: &str = r##"usage: herdr-subagents read [-h]
                            [--last | --since-turn SINCE_TURN | --tail TAIL]
                            name"##;

/// Verbatim `herdr-subagents reset --help` help.
pub const RESET_HELP: &str = r##"usage: herdr-subagents reset [-h] name

Reset a live subagent's harness conversation context.

positional arguments:
  name        live agent name

options:
  -h, --help  show this help message and exit
"##;

/// Usage line(s) of `herdr-subagents reset --help`.
pub const RESET_USAGE: &str = r##"usage: herdr-subagents reset [-h] name"##;

/// Verbatim `herdr-subagents status --help` help.
pub const STATUS_HELP: &str = r##"usage: herdr-subagents status [-h] [name]

Status of live subagents.

positional arguments:
  name

options:
  -h, --help  show this help message and exit
"##;

/// Usage line(s) of `herdr-subagents status --help`.
pub const STATUS_USAGE: &str = r##"usage: herdr-subagents status [-h] [name]"##;

/// Verbatim `herdr-subagents down --help` help.
pub const DOWN_HELP: &str = r##"usage: herdr-subagents down [-h] [--grace GRACE] [--all-dead] [--force] [name]

Stop a subagent or reap dead ones.

positional arguments:
  name

options:
  -h, --help     show this help message and exit
  --grace GRACE  seconds to let a turn finish
  --all-dead     gc every dead agent and exit
  --force        retire NAME without probing or closing its presentation
                 backend; archives state loudly
"##;

/// Usage line(s) of `herdr-subagents down --help`.
pub const DOWN_USAGE: &str =
    r##"usage: herdr-subagents down [-h] [--grace GRACE] [--all-dead] [--force] [name]"##;

/// Verbatim `herdr-subagents turn --help` help.
pub const TURN_HELP: &str = r##"usage: herdr-subagents turn [-h] [--message-file MESSAGE_FILE] [--cwd CWD]
                            [--model MODEL] [--fresh] [--effort EFFORT]
                            [--harness {codex,agy,muse}]
                            [--backend {tmux,herdr}] [--mode MODE]
                            [--timeout TIMEOUT] [--tail TAIL] [--quiet]
                            name [message]

Run one subagent turn to completion

positional arguments:
  name
  message

options:
  -h, --help            show this help message and exit
  --message-file MESSAGE_FILE
  --cwd CWD             required only when creating the agent
  --model MODEL
  --fresh               clear the agent's session first: a NEW TASK gets clean
                        context. Costs nothing (every turn re-invokes codex
                        anyway) and avoids carrying an unrelated task's
                        context into this one.
  --effort EFFORT       per-turn reasoning effort; omitted uses
                        SUBAGENT_EFFORT or the harness default
  --harness {codex,agy,muse}
  --backend {tmux,herdr}
  --mode MODE           headless (default for this path) or tui
  --timeout TIMEOUT
  --tail TAIL           lines replayed when --quiet
  --quiet               suppress live streaming; replay a tail at the end
"##;

/// Usage line(s) of `herdr-subagents turn --help`.
pub const TURN_USAGE: &str = r##"usage: herdr-subagents turn [-h] [--message-file MESSAGE_FILE] [--cwd CWD]
                            [--model MODEL] [--fresh] [--effort EFFORT]
                            [--harness {codex,agy,muse}]
                            [--backend {tmux,herdr}] [--mode MODE]
                            [--timeout TIMEOUT] [--tail TAIL] [--quiet]
                            name [message]"##;

/// Verbatim `herdr-subagents inbox --help` help.
pub const INBOX_HELP: &str = r##"usage: herdr-subagents inbox [-h] [--inspect | --drain] [--clear-composer]
                             name

Inspect or drain a TUI subagent inbox.

positional arguments:
  name

options:
  -h, --help        show this help message and exit
  --inspect         show pending and quarantined sequence numbers
  --drain           retry pending messages and skip poison entries
  --clear-composer  discard the visible TUI draft before draining; use only
                    for known stuck delivery
"##;

/// Usage line(s) of `herdr-subagents inbox --help`.
pub const INBOX_USAGE: &str = r##"usage: herdr-subagents inbox [-h] [--inspect | --drain] [--clear-composer]
                             name"##;

/// Verbatim `herdr-subagents up --help` help.
pub const UP_HELP: &str = r##"usage: herdr-subagents up [-h] --cwd CWD [--model MODEL]
                          [--harness {codex,agy,muse}]
                          [--backend {tmux,herdr}] [--mode {headless,tui}]
                          [--brief BRIEF] [--brief-file BRIEF_FILE]
                          name

Bring up a subagent.

positional arguments:
  name

options:
  -h, --help            show this help message and exit
  --cwd CWD             working root for the agent
  --model MODEL         default model (omit = harness default)
  --harness {codex,agy,muse}
  --backend {tmux,herdr}
  --mode {headless,tui}
                        per-agent mode override (otherwise environment/project
                        default)
  --brief BRIEF         first-turn brief text
  --brief-file BRIEF_FILE
                        file holding the first-turn brief
"##;

/// Usage line(s) of `herdr-subagents up --help`.
pub const UP_USAGE: &str = r##"usage: herdr-subagents up [-h] --cwd CWD [--model MODEL]
                          [--harness {codex,agy,muse}]
                          [--backend {tmux,herdr}] [--mode {headless,tui}]
                          [--brief BRIEF] [--brief-file BRIEF_FILE]
                          name"##;

/// Verbatim `herdr-subagents migrate --help` help.
pub const MIGRATE_HELP: &str = r##"usage: herdr-subagents migrate [-h] [--to {tmux,herdr}]
                               [--mode {headless,tui}]
                               name

Migrate an idle subagent presentation backend.

positional arguments:
  name

options:
  -h, --help            show this help message and exit
  --to {tmux,herdr}
  --mode {headless,tui}
                        destination mode for supported non-Codex runners;
                        Codex is TUI-only
"##;

/// Usage line(s) of `herdr-subagents migrate --help`.
pub const MIGRATE_USAGE: &str = r##"usage: herdr-subagents migrate [-h] [--to {tmux,herdr}]
                               [--mode {headless,tui}]
                               name"##;
