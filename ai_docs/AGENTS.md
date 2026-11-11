# ai_docs: rules for this directory

This file is the single source for `ai_docs/` conventions; other guides point
here instead of restating them. The parent `AGENTS.md` still applies.

## What belongs here

- Durable text: design notes, investigations, audits, measurements and
  records that must outlive a session. A durable finding goes in a GitHub
  issue, a pull-request comment, or a tracked file here.
- Small machine-readable evidence (CSV, TSV, JSON, short logs) that a
  document cites. Keep each file small.
- Not here: product code, tests, or product docs (those live in
  `common/docs/`); binaries, images, archives or bulk raw logs. Put large or
  disposable output elsewhere and commit a short text manifest instead.

## Naming and layout

- Put the date in every new file name: `YYYY-MM-DD-<topic>.md` (preferred),
  or `<topic>-YYYYMMDD.md` for a doc tied to an issue or pull request.
- One level of subfolders; the root holds only `README.md`, `AGENTS.md`,
  `CHANGELOG.md` and `CHANGELOG-PROTOCOL.md` (this repository currently
  uses only `README.md` and `AGENTS.md`). File every new doc directly into
  its cluster: `plans/` (designs, proposals, plans), `reports/`
  (owner-requested studies, audits, surveys; with `reports/research/` for
  investigations and studies and `reports/measurements/` for benchmark and
  timing results where that distinction holds), `reference/` (durable
  recipes, policies, how-it-works), `archived/` (finished episodes,
  including `archived/evidence/`, `archived/handoffs/`,
  `archived/records/` and `archived/transient/`), and `superseded/`.
- `archived/transient/` holds dated situational notes that are not
  maintained. Do not cite them as current guidance.
- `transient/` holds only one-line redirect stubs left where a document
  moved, so links posted earlier keep working. Add no new documents there.

## Moving and superseding documents

- Move a document with `git mv`, rewrite every link to it in tracked
  markdown, and leave a one-line redirect stub at its old path when that
  path may have been linked from outside this directory.
- Keep only the latest current doc on a topic in its cluster. When a newer
  doc replaces an older one, move the old one into `superseded/` in the same
  commit and note what superseded it.

## Writing style

- Cite issues and pull requests by number and slug, or by full URL.
- Numbers carry their denominator, date and the command that produced them.
- Never commit secrets, and never name a consuming project in this tree:
  describe the behaviour, not the client.
