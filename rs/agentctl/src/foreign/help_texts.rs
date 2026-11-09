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
