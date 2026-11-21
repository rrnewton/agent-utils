---
title: 'vibe-talk-discuss-glossary-per-channel: summaries need a project glossary per channel'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T13:55:00-05:00
---

# Description

The owner: "What the hell have we not provided any glossary at all to our summary agent. This is crazy that it would not know about detcore. That would be in any possible project glossary that we could come up with". The summariser's warm-up glossary covers only one project's terms, for every channel.

# Design

Per-channel glossary selection (channels or name patterns mapped to glossary files, plus a default list), back-compatible with the single warm-up list; a daily harvest of the unknown-terms log into a reviewed proposal of additions, never auto-applied.

Server shape (this change): `[summaries] catchup_warmup_files` stays the default list for every channel. New `[[summaries.catchup_glossaries]]` rules each name `channels` (channel ids or `*` name patterns matched case-insensitively against the channel id and its configured label) and `files`. `catchup::select_warmup_files` resolves one channel to the defaults followed by every matching rule's files in configuration order, each file once; with no rules the result is exactly today's list. The Discuss handler loads that resolved list through the existing windowed warm-up path, so fingerprints, budgets and glossary relevance selection are unchanged. A rule with no channels or no files is refused at load. The new field is marked as changing no summary wording in the summariser policy test, like the existing warm-up fields.

Daily loop (proposal only in this change; build follows approval): harvest the unknown-terms log, dedupe against the served glossaries, and write a proposal file (term, frequency, channels, suggested definition) for coordinator review. It never edits a live glossary. Proposed home: a user timer with a read-only ops script beside the other host ops timers, reading the log and glossary files and writing one dated proposal file for review. A task inside the chat server is the alternative; it is not proposed, because the review hand-off and the multi-glossary dedupe live with the coordinator's files, not with the serving path, and the loop must keep working while the server is down for its own restart.

# Acceptance Criteria

- A channel loads its project's glossary.
- The harvest writes a proposal file; tests are deterministic.

Progress: (a) a draft project glossary covering the five owner-flagged terms plus the core vocabulary of that project, with every logged unknown term listed with a proposed definition or a skip, is with the coordinator for the project lead's confirmation; it is operator data and is not committed here. (b) the server selection above, with unit tests for back-compatible resolution, pattern matching, ordering, dedupe and config parsing/refusal. (c) proposed above, awaiting the coordinator's go before any timer or script is built.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
Owner request, verbatim: "What the hell have we not provided any glossary at all to our summary agent. This is crazy that it would not know about detcore. That would be in any possible project glossary that we could come up with. Terms I didn't understand (5): Detcore, LiteInst, Reverie, MSEAL, seed frontier"
