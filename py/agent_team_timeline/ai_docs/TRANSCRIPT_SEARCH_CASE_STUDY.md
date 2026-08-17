# Transcript search case study: backend maturity

The Hermit archive provides a concrete acceptance test for transcript search: find where agents
defined backend maturity grade B3 and identify which backend measurements justified that label.
This is a zero-model, post-ingestion task; search must operate on verbatim normalized messages.

## Ground-truth records found

- `orc-coord-014`, 2026-07-25 22:17:40Z,
  `message:orc-coord-014::orc-note-4fb50e87-6969` defines the rubric: B2 runs trivial programs
  through real Detcore, B3 passes at least 50% of the ptrace strict-verify corpus, and B4 reaches
  100%.
- `orc-coord-014`, 2026-07-25 21:53:27Z,
  `message:orc-coord-014::orc-note-4fb50e87-6939` reports KVM at 134/180 strict-verify cells
  (74.4%), versus ptrace at 179/180, and calls KVM “comfortably at ~B3.”
- `claude-coord-176`, 2026-08-07 02:38:34Z,
  `message:claude-coord-176::251c77c1-0a56-4f4d-aa88-74b2293978e7:assistant-0`
  reports a controlled KVM repair measurement: 139/200 self-deterministic cells (69.5%) and
  131/177 measurable stdout-parity cells (74.0%).
- `orc-coord-014`, 2026-07-25 11:05:56Z,
  `message:orc-coord-014::orc-note-4fb50e87-6557` explicitly keeps DBI at B2+ because its full
  ptrace-corpus percentage had not been measured.
- `orc-coord-014`, 2026-08-07 06:33:23Z,
  `message:orc-coord-014::orc-note-4fb50e87-22179` later invalidates an earlier DBI B3 figure:
  DBI matched only 4/84 of its own detlog ordinals across two runs, so a quoted 130/152 (85.5%)
  result was not comparable until its dimension and normalization were explained.
- `orc-coord-030-hermit3`, 2026-08-13 01:49:36Z,
  `message:orc-coord-030-hermit3::orc-note-4fb50e87-s1-713` calls LiteInst B3 rather than B4
  after `/bin/echo` and `/bin/true` passed canonical L2 while `/bin/cat /dev/null` still failed on
  virtual-time differences.

## What the previous search path got wrong

The compatibility `--scope transcripts` query searched rendered phase-detail files, not a
phase-independent transcript corpus. Phase construction also used a final event timestamp as an
exclusive phase end, which omitted 240 final Codex assistant responses in the measured archive.
Broad searches silently returned only the oldest 50 matches without reporting the total. The
backend-maturity evidence was therefore discoverable only by slow, noisy scanning and some exact
messages were absent.

The new search corpus fixes those structural problems: it indexes normalized events directly,
keeps stable `message:` references, reports total versus returned matches, distinguishes owner
prompts and agent responses, and classifies parent instructions separately from child final
responses. Phase end derivation now retains the final millisecond event.

## Remaining limitations exposed by this case

1. **B3 is overloaded.** The archive has hundreds of whole-term `B3` matches because agents also
   use B3 as a checklist or review-item label. Lexical search cannot infer that the user means the
   backend maturity rubric. `backend maturity B3`, `B3 ptrace`, or `B3 strict-verify` is much more
   selective. A future semantic/concept layer could group rubric references without replacing
   exact lexical search.
2. **Short queries cannot use the trigram prefilter.** `B3` is two UTF-8 bytes, so it safely scans
   every selected team/day object. Three-byte terms such as `KVM` and longer combined queries can
   prune definite-miss shards.
3. **Provider routing is not uniformly explicit.** Claude instruction/final-answer messages are
   directionally clear. Codex paths and Orc task notes sometimes lack an unambiguous instruction
   partner; those records remain searchable but may have no `prompt_ref`. The UI must not invent a
   parent prompt from prose.
4. **A sliced export may omit linked context.** A response can retain a `prompt_ref` whose prompt
   falls outside the exported time range. `prompt_in_scope` records that condition; clients should
   show a clear unavailable marker rather than treating it as corruption.
5. **Raw tool payloads are intentionally absent.** Search indexes verbatim textual events and a
   condensed one-line tool record, not shell commands or tool output. Evidence present only in raw
   tool output is outside this corpus and needs a separately permissioned diagnostic surface.
6. **Exact terminology still matters.** DBI was later renamed DBT, and SaBRe has distinctive
   capitalization. Search currently performs ASCII-case-insensitive lexical matching but no synonym
   expansion. A curated glossary could provide explicit aliases without silently broadening exact
   results.
7. **Old websites require a rebuild.** Archives generated before the transcript search catalog
   continue to support compatibility phase search, but `--in` fails with a direct rebuild message.

## Scale observation

The seven-team Hermit snapshot used for this audit has 268,463 search records: 129,000+ textual
events and 139,273 condensed tool records. Canonical compact JSON measured about 301 MB identity and
50.4 MB at gzip-6 before per-day object overhead. The team/day Bloom catalog is therefore important:
selective queries avoid downloading and parsing objects that definitely cannot match, while broad or
short queries still pay the full exact-search cost. Tool-run grouping or a separate tool corpus is a
possible future optimization if real-browser measurements show that 139,273 compact tool records
dominate latency or memory.
