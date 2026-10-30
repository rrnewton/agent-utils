# local-ci-hub

Validation on a dev box (or several), with local and remote evidence
counted in one append-only ledger.

## Quickstart

`local-ci-hub quickstart` prints the five-minute local-only loop:
configure `local-ci-hub.toml`, `doctor`, `run start`, then
`receipt show HEAD` and `ledger query`.

The CLI help is the primary operator documentation; every
subcommand accepts `--json`.
