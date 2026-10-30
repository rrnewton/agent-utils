# local-ci-hub

Validation on a dev box (or several), with local and remote evidence
counted in one append-only ledger.

Objects: `run` (executions here), `ledger` (evidence), `receipt`
(verdict for one exact commit), `remote` (GitHub read/dispatch),
`health` (configured probes), `land` (lock + ancestry verify).

Start with `local-ci-hub quickstart` and `local-ci-hub userguide`.
Configuration is one `local-ci-hub.toml` in the project root; a
local-only project (`remote.provider = "none"`,
`ledger.publish = "none"`) needs no network or credentials.
