---
title: 'vibe-talk-speech-short-links: speak internal and other links as short phrases, never in full'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- speech
- ui
created_at: 2026-11-20T17:37:49-05:00
updated_at: 2026-11-20T17:37:49-05:00
---

# Description

[watermelon-full-preview] Owner 278, paraphrased: the read mode was shortening GitHub URLs just to say a GitHub URL, which was great, but internal company links are still read out in full, which is tedious and useless. (The owner's words name the company's internal domains; this is a public repository, so they are not quoted here.)

The existing speech-text rule (`vibe-talk/src/speakable.rs` `say_address`, with the GitHub table in `vibe-talk/src/link_labels.rs`) says a labelled GitHub page as its page ("widgets pull request 41") and any other addressed host by its host alone, but it only recognises a bare schemeless address for `www.` and the GitHub host, and it has no rule for internal company links: a schemeless internal address, and a device-voice document read (the Documents reader's speech-text helper, `documentReadBlocks`/`speakable` in `vibe-talk/web/src/view/documents/blocks.ts`, whose texts are spoken as written), can still read an internal URL out in full, path and all.

# Design

- Speech text only: what is spoken changes, never what is displayed. The GitHub display labels (`short_label`) and every drawn link are untouched, and the existing GitHub spoken behaviour is pinned by its current tests, unmodified.
- No internal host is named in code (this repository is public). The internal-host list comes from the deployment's config: `[speakable] internal_link_hosts` on the server (empty by default, checked at load), sent to the page in client-config (`internal_link_hosts`), so the server speech pass and a device voice on the page shorten the same links. The real hosts are deployment configuration, outside this tree.
- A table-driven spoken rule for configured hosts: where the path makes the kind obvious, say the kind and id ("paste P123", "diff D123", "task T123", with a long numeric id said by its ending, as tracker ids already are); otherwise "an internal link".
- A bare schemeless address on a configured host is recognised as an address (as the GitHub host already is), so it is shortened rather than read out.
- Any other URL not covered by a special rule keeps the host-only rule already pinned in the speech tests ("a link on example.com", a bare host as itself).
- The same spoken shortening in the reader's speech-text helper (`blocks.ts`), with hosts read from a provider the Documents view registers from client-config; a link with a writer's own label keeps that label.
- Tests use fixture strings only: fake hosts (for example "intranet.example") configured in the test, as a deployment would, and obviously fake ids. No real internal URL, host or id is committed anywhere.

# Acceptance Criteria

- With fake hosts configured, a bare or URL-labelled internal link is spoken as "paste P123", "diff D123" or "task T123" where its path names that kind and id, as the kind-by-ending for a long numeric id, and as "an internal link" where the path names no obvious kind.
- A schemeless address on a configured host is shortened the same way, not read out in full; with no hosts configured, the same address takes the generic host rule.
- Any other uncovered URL is spoken as its host only; a bare host is spoken as itself.
- Every existing GitHub spoken-label test passes unmodified, and no displayed label or drawn link changes.
- A grep of the full change for the company's internal host names gives zero hits.
- Focused suites pass: the Rust speech and config tests, the contract tests, and the JS documents-blocks, documents and settings tests, with new fixture-only cases for the internal and generic rules in both speech-text functions.

# Notes

[watermelon-full-preview] Filed by mu-urlspeech at coordb's assignment on 2026-11-20; number pre-assigned by coordb. Owner fast-track. Coordination: mu-readstart (#2003/#2004) is working in the reader — messaged before any read-aloud.ts change; this lane touches no reader or read-aloud files, only the speech-text functions and their config plumbing. No live server, no real internal URLs or ids in fixtures. The deploying operator adds the real hosts to the live config outside this repository.
