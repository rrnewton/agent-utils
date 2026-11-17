# wrkslots schema-level version skew — design (one page, for review before coding)

Issue: wrkslots-schema-level-version-skew. Status: design only; no code yet.

## Problem

An older pinned reader refuses a registry a newer writer touched, because the
strict key-set check calls any unknown key corruption. The log is append-only,
so one newer event bricks every older reader permanently. Two recent additive
fields did this: handoff accounting (ignored-file/archive evidence) and a
seal-journal filesystem id. The archive-only precedent quarantines one unknown
archived row and loads the rest; the agentctl precedent tolerates unknown keys
in persisted files (warn once, keep through rewrite) and keeps authority
records strict; journals and config already carry a minimum-version floor and
an extension slot. Events, active records, handoffs, seals, receipts and
manifests have none of that.

## Proposal

1. **Every persisted history carries its level.** Each envelope (event,
   active/archive state, handoff sidecar, journal, receipt, manifest) gains a
   writer schema level plus writer version, written on every new write.
   Existing files without it read as level 1 (today's shapes). The level only
   ever rises; a reader never rewrites history at a lower level.

2. **Per-kind tolerance, decided by decision-relevance.** A reader meeting a
   newer level either (i) accepts additive optional fields it can ignore for
   the decision at hand, or (ii) refuses with a typed "newer writer" refusal
   naming writer version and minimum reader version — never "invalid fields".
   Ignorable means: absent-vs-default is unobservable to every guard the
   reader runs (identity, containment, digest, accounting of deleted bytes).
   Decision-relevant means: ignoring it could authorise deleting, sealing or
   trusting bytes the field describes. New optional data goes under a per-kind
   extension key (`x`/`ext`, following the journal precedent), never as a new
   top-level required key; required-key additions always bump the level.

| Record kind | Ignorable (accept, keep, warn once) | Must refuse (typed skew) |
|---|---|---|
| Event envelope + payload | extension keys; audit-only annotations | new `kind`; payload keys feeding replay state |
| Active record + checkouts | extension keys; display-only metadata | checkout identity/containment; provisioned digests |
| Handoff (+ accounting) | narrative limitations; extension keys | ignored-file/archive accounting that `remove` enforces; `discarded_unaccounted` |
| Journals incl. seal | extension slot (existing rule); phase annotations | new phase; seal identity/fs-id inputs to restore/fence |
| Archive rows | contained unknown layout (existing quarantine) | checkout outside every historical candidate |
| Receipts / manifests | extension keys; extra artifact descriptors | digests, sizes, ref targets that verification compares |

   Note the trap in the reported defect: the new handoff accounting *looks*
   additive but `remove` enforces it exactly, so under this table it is
   decision-relevant — an old reader must get the typed skew refusal (or the
   writer must not have emitted it; see 3), not silent tolerance.

3. **Writer restraint (protects already-deployed old pins today).** A writer
   emits a post-floor field only when the project's declared reader floor
   covers it. `.wrkslots.yml` gains `reader_floor`/`writer_ceiling` beside the
   existing minimum-version key: floor = oldest reader that must keep working,
   ceiling = highest level this project allows writers to emit. Default:
   ceiling = floor = current level, so nothing new is emitted until the
   project opts in. `init --repair` is the only deliberate raise (named,
   reported, never silent); a writer above ceiling refuses with the remedy.
   This is what stops new fields bricking old pins *now*, before old readers
   can be changed.

4. **Parity and proof.** Python and Rust share one per-kind key table (the
   existing schema mirror), one skew-refusal wording, and differential
   fixtures: old-reader/new-writer for events, journals, handoffs (plus
   archive quarantine regression). Tests fail before, pass after; condition-
   based waits only.

## Asks of the reviewer

Approve (a) the per-kind table above (esp. handoff accounting = refuse, not
ignore), (b) floor/ceiling names + `init --repair` as sole raiser, (c) typed
skew refusal as a distinct exit/message from corruption. Then coding starts:
writer gate first (stops the bleeding), reader tolerance second, fixtures last.
