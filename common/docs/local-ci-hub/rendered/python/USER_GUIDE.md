local-ci-hub userguide
Objects: run (executions on this machine), ledger (append-only evidence),
receipt (verdict for one exact commit), remote (GitHub read/dispatch),
health (configured probes; scheduling is external), land (lock+verify).
Config: local-ci-hub.toml in the project root. [validate] command is yours;
[ledger] location/team/publish (none|git); [remote] provider (none|github);
[receipt] qualifying is an AND list from {local-ledger-pass, remote-pass}.
Local-only: provider=none and publish=none needs no network or credentials.
Every subcommand accepts --json. Exit codes: 0 ok/qualified/pass, 1 not-qualified/fail, 2 usage/config error.
