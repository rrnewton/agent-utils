# Phase 4: template or "set me up" recipe (draft, no repository created)

Question: what should a new adopter get — a template repository, or
nothing but a sentence to their agent? Owner's framing: make it as
thin as "set me up a multi-agent workspace using agent-utils",
modeled on the originating deployment (consumer-a), stripped of its
cruft. Names here are neutral labels; no consuming project is named.

## Answer up front

A recipe, not a template repository, for everything the suite
already automates; a template only for the residue the recipe
cannot express. Measured against the suite's existing setup skill,
the residue is three small files. If those three files can be
generated from answers at setup time, the template repository
should not exist.

## What "set me up" already produces today

The suite's setup flow (coordinator harness, slots, launch
profiles, linked skills, harness AGENTS.md) is already a recipe:
one sentence, detection first, one batch of questions, then the
work. Adding local-ci-hub to that flow needs exactly:

1. One extra setup question: "What is this project's validation
   command?" (default: none yet — the tool then runs in
   ledger/receipt-only mode against whatever the project adds).
2. One generated file: `local-ci-hub.toml` at the project root,
   with `remote.provider = "none"` and `ledger.publish = "none"`
   until the adopter opts into a remote or a published ledger.
3. One cron/timer line the adopter may accept: the shadow health
   probe, off by default, exactly like consumer-a's flag.

That is the whole footprint. Everything else (admission, ledger
format, receipt verdicts, land primitives) ships with the tool.

## The minimal template, if one is still wanted

If a template repository is created later (owner decision, not
made here), it should contain only what a recipe cannot generate:

| File | Why it cannot be generated |
|---|---|
| `local-ci-hub.toml` (annotated example) | Adopters need a commented example to edit, not a blank schema |
| `ci/README.md` (one screen) | The local conventions: who lands, what qualifies, where the ledger lives |
| `.gitignore` additions | Ledger spool and run state must never be committed by accident |

Explicitly not in the template: harness layout, launch profiles,
skills, health probes, landing scripts. Those are either generated
by setup or are the adopter's own policy, and freezing consumer-a's
versions of them into a template is how cruft propagates.

## What the recipe must ask (and must not)

Ask, once, with defaults: project name; validation command;
whether a remote exists and whether its results should count
toward qualification (default: report only, never qualify);
whether the ledger is published to git (default: local only);
who may land (default: a human runs the land command).

Never ask: host names (short host is derived), team name (defaults
to the project name), slot counts (sized from the machine), digest
tooling (delegated only when the project already has a canonical
one). Every question the recipe asks is one the tool cannot answer
from the checkout.

## Thinness test

The recipe succeeds if an adopter goes from a bare checkout to
`doctor` green, one `run start`, and one `receipt show` in under
five minutes with no file edited by hand — the same toy-loop
budget phase 2 was held to. If a step needs hand-editing, that
step is a template candidate; if none do, there is no template.

## Open decisions for the owner/coordinator

1. Does the setup skill gain the local-ci-hub question now, or
   does the tool stay opt-in per project first?
2. Ledger publishing default: local-only (proposed) versus
   git-published from day one (consumer-a's current mode)?
3. If a template repo is wanted after all, who owns keeping its
   three files in step with the tool's config schema?
