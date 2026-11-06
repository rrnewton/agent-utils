---
title: 'wrkslots-remote-url-equivalence: treat ssh and https spellings of the same repository as an unchanged remote'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T04:24:40.989072520+00:00
updated_at: 2026-11-05T04:24:40.989072520+00:00
closed_at: 2026-11-06T19:18:43.000000000+00:00
---

# Description

`wrkslots unpushed`, `remove` and `finish` refuse with "checkout NAME remote origin URL changed" when a slot was created while the checkout's origin used one transport, and the shared source has since switched to the other: an ssh URL (`git@host:owner/repo.git` or `ssh://git@host/owner/repo`) to https (`https://host/owner/repo.git`), or back. Both URLs name the same repository, so nothing about where the work is published has changed.

On the lead host this blocks inspection and reclaim of every slot created before a consumer team's ssh-to-https switch. A consumer team tracks it in its own tracker.

Cause: the slot record keeps only `remote_url_sha256`, a hash of the raw URL string. The comparison in `py/wrkslots/cli.py` (`remote_authority(...).sha256 != checkout.remote_url_sha256`, at five refusal sites) does no normalization.

# Design

- Add one helper for the comparison, used at every "URL changed" site. It accepts when the recorded hash equals the hash of the current URL OR of one of its equivalent spellings for the same host and path: scp-style ssh, `ssh://` with or without the `git@` user, and https, each with and without a trailing `.git` or `/`, with the host compared case-insensitively.
- This works for existing records, which store only the hash, because the candidate spellings are generated from the CURRENT URL and hashed.
- Stay strict about anything else. A different host, owner or repository path, a local path, or a `file://` URL is still a change. Never treat an http (plain) URL as equivalent.
- When the check accepts an equivalent spelling, say so in the output ("origin URL respelled ssh→https; same repository"), and record the new hash on the next write of the slot record, so that later checks are exact.

# Acceptance Criteria

- Unit tests cover:
  - recorded ssh scp form vs current https, and the reverse;
  - with and without `.git`;
  - host case;
  - `ssh://` with and without a user.
- Negative tests cover a different owner, a different repo, a different host, and a local path; all are still refused.
- An end-to-end test creates a slot, switches the source origin from ssh to https, and shows that `wrkslots unpushed` and `remove` proceed.

# Notes

Requested by the lead on 2026-11-04. Assigned to mu-validate after #864 (the same wrkslots salvage area); reassign to whichever lane frees first.

Moved from mu-validate to the new Muse ultra wrkslots lane mu-wrkslots on 2026-11-05 at 5:50 PM ET (the owner lifted the Muse lane cap; the lead asked for a dedicated wrkslots lane).

## Resolution

Landed in the mu-wrkslots stack at 84585684. ssh/https/scp respellings of the same host and path are accepted at the URL-changed refusal sites (recorded digest hashed against generated spellings), with a respelling note in remove output; local paths, file://, plain http, and different host/owner/repo still refuse. Covered by the spelling unit matrix; a network end-to-end is not fixture-feasible since the spellings name unreachable hosts by construction.
