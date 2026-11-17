---
name: landing-modes
description: "Coordinator-level landing-mode protocol: gated, optimistic, composed, emergency repair and review-gated landing, where each is legal, the evidence each needs, and how to name the mode in a hand-back. Inactive unless the project owner enables it; a project that has not enabled it states only that its main is gated. Use when an owner asks for landing modes, or when deciding how an owner-controlled branch reaches a shared main."
---

# Landing modes

**Inactive by default.** A project uses this skill only when its owner enables
it. A project that has not enabled it says one thing in its own guidance: its
main is gated. This is coordinator guidance for choosing how a lane's work
reaches main; it is not guidance for an agent working inside a product
repository, and product repositories do not restate it.

## Name the mode, every time

| Mode | Where legal | Gate |
| --- | --- | --- |
| **Gated** | a shared main | main advances only to a commit the landing gate (a merge queue or equivalent) validated green, so main moves green to green |
| **Optimistic** | a branch one owner-controlled loop owns end to end (an integration branch, a lane stack, a feature branch) | approval plus targeted tests at the exact head; full validation afterwards, fix forward; never on a shared main |
| **Composed** | an optimistic branch reaching a shared main | accumulate optimistically, then send the whole branch through the gated path as one unit |
| **Emergency repair** | a red shared main, only where the owner authorized it | a direct landing scoped to the repair that restores green, declared as an emergency in the hand-back |
| **Review-gated** | a forge that lands one reviewed change at a time | land by the change's explicit number and reviewed version once its required reviews approve that version and its own CI shows nothing land-blocking |

"Soft green" named the optimistic gate and was read as permission to land a
shared main that way. It is not a landing claim: say the mode instead. Code
identifiers that contain the phrase are only names.

## Optimistic-mode evidence

- **Chain-tip receipt.** Required review plus a full validation at the chain
  tip of the lane branch, with the receipt in the hand-back. It carries through
  a clean rebase without re-validating; targeted tests are the inner loop only.
  Conflict resolution or substantive edits are not clean-rebase carry: review
  the changes and requalify the tip.
- **After landing.** Eagerly full-validate the new tip. A byte-identical tree
  inherits the matching receipt. Pipeline full validations with a configurable
  concurrency K, starting at K=1; localize red results with targeted runs and
  bisect.
- An approval binds an exact head; a head move needs a delta review unless the
  move is a clean rebase.

## Turning it on

The owner enables the skill per project and names the modes that project may
use; the project's own landing guidance then links here and records which
branches are optimistic. Until then, ignore this skill and follow the project's
gate.
