# local-ci-hub (Rust)

Rust successor to the Python local-ci-hub ledger core (#1365
rust-ci-hub-ledger). One storage-agnostic core — typed events, one fold, one
qualifying predicate, one canonical digest — behind a fan-out `Store`
contract with one read authority per deployment, a disposable SQLite index,
and the existing six-object CLI.

M1 scope: typed core events with a refusing union and fold, the versioned
SQLite index with a rebuild-from-authority test, the `Store` trait with the
git least-common-denominator connector, read-only `receipt show`, and the
versioned `ci-hub-store-wire/v1` contract (implementation in M2). Python
remains the primary edition until parity and shadow gates pass; this crate is
reached with `cargo run -p local-ci-hub` meanwhile.

See `common/docs/local-ci-hub/` for operator documentation and the wire
contract.

## Deployment

The front door is one binary for every ledger. Everything that differs
between deployments — where the ledger lives, which head to fold, which
field names the deployment's rows use, which producers are fixtures — is
data, supplied as flags or as one JSON config file. No deployment value is
compiled in. Record the deployment's values once in a config file, keep
that file with the deployment (not in this repository with real values),
and every later run replays exactly them.

Copy `deployment.example.json` in this directory and replace every
`<...>` placeholder with the deployment's values:

```bash
local-ci-hub --config deployment.json ledger rows --terminals
local-ci-hub --config deployment.json ledger verdicts
local-ci-hub --config deployment.json ledger digest-rows
local-ci-hub --config deployment.json receipt show <commit>
```

`--config FILE` reads JSON with keys `repo`, `git_ref`, `evidence_root`,
`binding_tree_field`, `base_fields`, `fixture_producers`,
`evidence_head_field` and `index`. Every key is optional; `null` for
`evidence_root` or `index` means "keep the default". Precedence, highest
first, is: an explicit flag, then the config file, then the built-in
default. The two list keys work the same way as the repeatable flags they
stand for: when any `--base-field` flag is given, the flags alone win;
otherwise the config list is used whole. An unreadable file, invalid JSON,
an unknown key, a wrong-typed value or an empty field name is a usage
error (exit 2) naming the config; nothing is silently ignored, because a
silently dropped deployment value folds the wrong ledger and still exits
as if it had succeeded.

The same values can be passed as flags without a config file. The flags
below are global: they may appear before or after the subcommand. Each
entry gives the flag, its config key, its default, what it means, and what
breaks when the deployment value is missing or wrong.

- `--repo` (config `repo`, default `.`): the git repository holding the
  ledger shards. The git connector reads shard files from this
  repository's objects at `--git-ref`; it never writes to the repository.
  Pointing it at the wrong repository folds the wrong history, or no
  history at all: `receipt show` then reports `no-result` (exit 2) for
  commits the deployment has evidence for.
- `--git-ref` (config `git_ref`, default `HEAD`): the ref or commit whose
  git objects are the single read authority for the run. Every output
  line in one run folds this one head. Folding a different head than the
  oracle folded — a moving branch where the oracle used a pinned commit —
  makes row counts, verdicts and digests describe different inputs, so an
  acceptance comparison against that oracle fails even when both sides
  are individually correct. Acceptance always pins an explicit commit.
- `--evidence-root` (config `evidence_root`, default unset): the
  filesystem root of the retained-evidence archive; a stored row
  reference (`row-ref/v1`) names its object as a path under
  `<root>/run-evidence/`, and expansion authenticates the object (size
  and SHA-256) before the row exists. With no root configured, any
  ledger containing a row reference refuses the whole read with
  `no evidence source configured`. A root that does not hold the
  deployment's objects refuses as unreadable or digest-mismatched
  evidence. A ledger with no row references never consults the root, so
  local and synthetic ledgers need none.
- `--binding-tree-field` (config `binding_tree_field`, default `tree`):
  the `cell_results` field whose value is copied as `tree` into a derived
  evidence binding. Stored `cell-dedup/v1` lines omit most bindings; when
  a cell carries a `selected_attempt` but no stored binding and is not
  listed in `evidence_binding_absent`, expansion derives the binding
  from this field, the run id and the cell identity. If the deployment's
  field name differs from the value given here, derivation finds no tree
  and the whole read refuses with `cell N has no derivable evidence
  binding` (for example `cell 0 has no derivable evidence binding`).
  This is the most common first-run refusal: set the field name the
  deployment's `cell_results` objects actually carry.
- `--base-field` (config `base_fields`, repeatable, default adds
  nothing): extra row field names the qualifying predicate requires, on
  top of the built-in `base_sha` and `base_tree`. From schema 5 on, each
  named field must hold a 40-character lowercase hex string or the row
  refuses as `base-evidence-missing-or-invalid`. The list must be exactly
  the deployment's required base-evidence set. Omitting a required name
  lets rows without that evidence qualify, so verdicts can be looser
  than the deployment's oracle. Naming a field rows do not carry makes
  every schema-5 row refuse, so no commit qualifies.
- `--fixture-producer` (config `fixture_producers`, repeatable, default
  excludes nothing): producer tool names whose events are admission
  fixtures. After the union, an event whose envelope producer tool, or
  whose row producer, names a listed tool is excluded and counted on
  stderr (`filtered N fixture events`); it never becomes a row, a
  verdict or a digest. Omitting the deployment's fixture producers folds
  fixture runs as real evidence, inflating row and terminal counts and
  adding digests no oracle row matches. Listing a real producer excludes
  real evidence the same way, in the other direction.
- `--evidence-head-field` (config `evidence_head_field`, default
  `validated_head_sha`): the `cell_results` field naming the validated
  head inside typed cell evidence. A `cell_results` value carrying the
  exact typed shape (nine keys including this field, with typed
  artifact, selected identities and cells) canonicalizes in struct
  declaration order at every depth; that order is what the receipt
  digest hashes. **Get this one right before comparing digests.** With
  the wrong field name — including the default on a deployment whose
  evidence uses another name — the exact-shape check fails and every
  such value silently falls back to generic sorted-key form, so
  `ledger digest-rows` still prints a digest for every row, rows and
  verdicts still agree, and the only symptom is that the digests do not
  move: a front-door upgrade that was supposed to change them leaves
  the output byte-identical to the previous run. That unchanged output
  has already been misread once as "the typed canonicalizer is not
  running". As a guard, `ledger digest-rows` warns once on stderr when
  the configured field is absent from every typed `cell_results`
  record it checked, naming the field and the record count; no warning
  means at least one record carried the field (or no typed evidence
  was present at all).
- `--index` (config `index`, default ephemeral in-memory): the SQLite
  index file (`ci-hub-index` schema) the run tails the authority into
  before folding. Without it, every invocation cold-folds the whole
  authority and discards the index on exit: correct, but the full tail
  cost is paid per command. With a file, the first run builds it and
  later runs tail only new bytes; indexed, cold and warm outputs must
  be byte-identical, which the acceptance script checks. Deleting the
  file is always safe and forces a rebuild with identical outputs. An
  index records its authority and workspace, and an index at any other
  schema version is discarded and rebuilt rather than trusted.

One read, end to end: the binary tails the authority page by page,
expands stored codec lines (consulting `--evidence-root` and
`--binding-tree-field`), unions the pages, drops `--fixture-producer`
events, checks the envelopes, admits the rest to the index and refolds
once. `ledger rows` prints the folded rows, `ledger rows --terminals`
prints one terminal summary per row, `ledger verdicts` judges each
distinct commit with the predicate (built-in base fields plus any
`--base-field` names), `ledger digest-rows` hashes each row with the
typed canonical form selected by `--evidence-head-field`, and
`receipt show <commit>` judges one commit the same way. Exit codes: 0
qualified or printed, 1 not-qualified or refused, 2 no-result, usage or
cannot-tell, 75 could-not-run. A refusal (exit 1 with `refused:` on
stderr) is never a test result and never green.

## Acceptance

`scripts/acceptance.py` in this package compares the Rust front door
against a deployment's own oracle implementation, using only the
deployment config and three caller-supplied oracle commands. Nothing
about any deployment — no path, field name, producer name or oracle
location — is baked into the script.

```bash
python3 scripts/acceptance.py \
  --config deployment.json \
  --work-dir /path/to/scratch \
  --local-ci-hub /path/to/local-ci-hub \
  --oracle-fold '<command printing folded rows as JSONL>' \
  --oracle-verdicts '<command printing qualified commits>' \
  --oracle-digests '<command printing {"run_id","digest"} JSONL>'
```

The oracle commands are shell commands run with `sh -c`; each must print
its JSONL on stdout and exit 0. Folded oracle rows are reduced to the
same terminal fields the Rust `--terminals` output prints. The verdict
oracle may print either `{"commit": ..., "qualified": ...}` objects or
one bare commit id per line for each qualified commit. Digests compare
per `run_id`, including `null` (a row with no digest must be `null` on
both sides). The script checks, and fails (exit 1, naming the first
mismatches) unless all hold:

- folded terminal sets are identical;
- no commit the Rust side qualifies is unqualified in the oracle
  (looser must be zero; stricter commits are listed by id — each one
  needs a documented reason before the deployment is accepted);
- every shared `run_id` has the same digest on both sides, and the
  `null`-digest counts agree;
- index stability: verdicts cold (no index), verdicts building a fresh
  file index, and verdicts reusing that warm index are byte-identical.

Wall, CPU and memory figures are reported by the caller's timing wrapper
if wanted; the script asserts no timing property. The Rust JSONL stays
in the work directory for inspection; the caller owns its cleanup. The
script removes only a stale verdict index it is about to rebuild, never
the deployment's own index.
