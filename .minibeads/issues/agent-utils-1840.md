---
title: "vibe-talk-draft-sync-across-devices: sync composer drafts across devices"
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-18T08:17:51.000000000-05:00
updated_at: 2026-11-18T08:17:51.000000000-05:00
---

# Description

Owner (thread 172, verbatim): "Hmm could we SYNC drafts.. I just wrote a long half written message on my phone and kinda want to continue on laptop... on android do we see what is in the text entry box before the user presses enter? If so maybe we could have a timer that syncs the draft if it sits there unsent for a while. Or we could have another small button that says "save draft"..."

Today every composer draft lives only on the device that typed it: the page keeps channel and thread drafts in device storage at each keystroke (`web/src/core/outbox/drafts.ts`, the ledger in `draftledger.ts`), so a half-written message started on one device is invisible on another. The liaison told the owner this is feasible and suggested: auto-sync to the server after a few idle seconds (per channel or thread, with a quiet "Draft saved" note); the other device offers "Continue draft from <device> (N min ago)" instead of overwriting; an optional "Save draft" button. The owner may still say button-only or both, so the design covers both, with auto-sync behind a setting.

Yes, the page sees the text entry box before Enter: each composer (`compose-text`, `channel-compose-text`, `side-compose-text`, `reply-text`, `quick-reply-text`, `discuss-followup` in `web/src/view/composer/keys.ts`) reports its words as they are typed, which is what the device drafts already save.

# Design

Full design note sent to the coordinator before coding (storage, key, conflicts, clearing, Discuss scope, privacy and retention, API and auth, idle timer, offline behaviour). Summary:

- Storage: server-side on the owner's own vibe-talk server in a new sqlite table created OUTSIDE the migration ladder (`CREATE TABLE IF NOT EXISTS` in `SqliteStore::open`, exactly like `thread_class` in `vibe-talk/src/store/sqlite.rs`), so a rolled-back older binary still opens the database. Never in Slack, never in the synced-settings document (`client_settings`), whose module note deliberately keeps drafts out.
- Key: one row per (channel, thread or channel scope, composer scope) covering the main composer and the Discuss follow-up composer (`discuss-followup`); inline reply and quick-reply drafts stay device-local in this step.
- Conflicts: the server is the rendezvous, not an overwriter. A device with unsent words of its own is never silently overwritten: it is offered "Continue draft from <device> (N min ago)" with Use and Discard, and choosing neither keeps the local text. Newest-wins applies only to an empty or identical box. Sending clears the server draft; an explicit discard clears it too.
- Behaviour: an idle timer (a few seconds, injectable in tests with no wall-clock) auto-syncs when enabled, a "Save draft" button syncs at once, and a quiet "Draft saved" note confirms it. Drafts expire after a bounded retention, are capped in size and count, need the write credential (a read token gets 403, no MCP route reaches them), are never logged, and device storage stays the offline source of truth with a flush on reconnect.

# Acceptance Criteria

- A deterministic test: a draft typed on "device A" appears as the continue offer on "device B", and sending clears it (no wall-clock waits; the timer is injected).
- Two devices editing the same scope never silently lose typed words: the second device sees the offer and keeps its own text until it chooses.
- Sending a message deletes its server draft; explicitly discarding deletes it; an expired draft is never offered.
- A read-scope token cannot read or write drafts; drafts never reach Slack, the access log, or any MCP tool.
- Page tests at phone and desktop sizes in light and dark theme, keyboard open and closed, with the screenshot matrix on a test instance (phone and desktop, "Draft saved" note and "Continue draft" offer states).

# Notes

Owner-priority, deployed on landing. Owner request in thread 172 via the liaison; screenshot acceptance per thread 124 (test instance only, keyboard really open, Discuss half-pane in at least the phone shots). Design note to the coordinator precedes any code; the owner may still choose button-only, auto-sync only, or both, and the build follows that choice.

## Resolution

[watermelon-full-preview] Landed as e166ec38 on main (deployed on landing by the coordinator). Server: the `composer_drafts` sqlite table outside the migration ladder (7-day retention, size and count caps), a write-scope drafts API (a read token gets 403, no MCP route), the storage purge and a forgotten channel dropping drafts. Page: idle auto-sync behind the synced `vibe-talk.draft-autosync` setting (default on) plus Save draft buttons on the main and Discuss composers, a quiet "Draft saved" note, and "Continue draft from <device> (N min ago)" offers that never overwrite typing; sending or discarding clears the draft, and device storage stays the offline copy. Proof: Rust store and real-router tests (one device's draft offered on another, sending clears it, the purge drops it), a two-device page suite with injected timers, and a browser cross-device run on a test instance with the standing screenshot matrix.
