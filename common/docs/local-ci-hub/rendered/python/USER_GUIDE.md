local-ci-hub userguide
Objects: run (executions on this machine), ledger (append-only evidence),
receipt (verdict for one exact commit), remote (GitHub read/dispatch),
health (configured probes; scheduling is external), land (lock+verify).
Config: local-ci-hub.toml in the project root. [validate] command is yours;
[ledger] location/team/publish (none|git) and counted_repos (repository
identities this ledger counts; rows with no repo always count);
[remote] provider (none|github);
[receipt] qualifying is an AND list from {local-ledger-pass, remote-pass}
and release_payload is the canonical release identity a PASS must carry.
Host limits: one admin-installed host file caps concurrency, cleanup
debt, disk floor and per-run box ceilings for every instance on the
host; project requests only clamp down under it. With no host file the
tool runs on conservative built-in limp limits and warns on every run;
doctor reports the missing file as an error and shows every effective
value with its source. Admission is live, from measured headroom: a
refusal is no result, never a test failure. Sweep deletions defer
while a measurement hold or host quiet is active.
Local-only: provider=none and publish=none needs no network or credentials.
Every subcommand accepts --json. Exit codes: 0 ok/qualified/pass,
1 not-qualified or real test failure, 2 usage/config error,
75 could-not-run or refused (no result was produced).
