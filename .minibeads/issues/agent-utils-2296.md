---
title: 'land-main-no-gate-evidence-scope: refuse changes outside .minibeads and docs unless a logged override names the approver'
status: open
priority: 1
issue_type: task
assignee: mu-statesync
labels:
- land-main
created_at: 2026-11-26T10:30:59-05:00
updated_at: 2026-11-26T10:30:59-05:00
---

# Description

`bin/land-main --no-gate-evidence REASON` currently skips the gate-ledger evidence check for any source whose first eight hex digits appear as a substring in a local waiver file. That check does not parse who approved the override, require the full source or replayed commit/tree, bind the approval to its reason or range, record an expiry, or prove that the named line is an exact authorization record. Docs-only paths outside `.minibeads/` are also treated as code, while a substring match can authorize a code landing without machine-checkable evidence of the approver.

Two incidents show why the shortcut needs a hard scope. On 2026-11-20, a lane landed a hand-merged code tree through this path (the misuse already recorded in `bin/land-main`). On 2026-11-26, the #2280 S3 code commit `afb8ecaa` landed as `dce61a5b` with `--no-gate-evidence` under coordb's waiver because its only functional gate red was the pre-existing read-modes timeout (#2293); the liaison then refused that shortcut after the landing and kept the commit only on gated follow-up conditions. Even an authorized use therefore leaves the audit trail dependent on chat history and a substring in a local file rather than an exact, logged approver record.

# Design

1. Compute the exact set of paths the push will change for both direct and `--replay-base` landings, against the correct base and the replayed tip/tree. With `--no-gate-evidence`, refuse before any network operation when that set is empty, cannot be determined, or contains anything outside an explicit allowlist.
2. The no-override allowlist is `.minibeads/**` plus enumerated documentation paths only. Put those documentation roots in one place in the script and cover each with tests; at minimum distinguish `ai_docs/**`, `common/docs/**`, `site/**`, and Markdown documentation from code or configuration that merely has a documentation-like name. A mixed docs-plus-code range is refused without an override.
3. Outside that allowlist, require an explicit, machine-checkable, logged override record that names the approver. The record must be parsed as a record (not found by substring grep) and bind at least the approver, the full source SHA, the replayed commit SHA or tree when replay is used, and the reason/issue it authorizes. Missing, malformed, inexact, or reused-for-another-tree records refuse the landing. The printed warning must name the approver and the exact commit/tree authorized.
4. Preserve the issue-file-only fast path: an all-allowlisted commit needs no override record, only the existing reason printed by `--no-gate-evidence`. Do not weaken the ordinary gate-evidence path or the fast-forward, non-empty-source, full-SHA, overlap, and post-push fetch checks.
5. Test the guard directly against shell fixtures or the script's seams for issue-only, docs-only, mixed docs+code, code-only, deletions and renames, direct and replayed sources, an exact valid override, an override for another SHA/tree, a substring-only match, and a record with no approver.

# Acceptance Criteria

- `land-main --no-gate-evidence` refuses any exact pushed range containing a path outside `.minibeads/**` and the enumerated documentation paths unless a parsed override record names the approver and exactly matches the source (and replayed commit/tree when replayed).
- Substring or prefix-only waiver matches, unnamed approvers, malformed records, and records for another SHA/tree are refused before pushing; the refusal names an example disallowed path and what exact record is required.
- Issue-only and docs-only landings still succeed with no override record, including deletions/renames whose old and new paths are both allowlisted.
- Focused script tests cover the case matrix in Design item 5 and prove no push is attempted on every refusal path.

# Notes

Filed after the #2280 S3 waiver landing. The present substring waiver mechanism and its 2026-11-20 precedent are recorded in `bin/land-main` (lines 112-126 on the filing tree); this issue is to scope and harden that mechanism, not to re-litigate the already-landed commits. Related: `#2280 vibe-talk-state-sync-audit` Notes (the waiver landing and its conditions) and `#2293 vibe-talk-read-modes-held-walk-thread-option-timeout` (the pre-existing red behind the waiver).
