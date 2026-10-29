---
title: 'chat-receipt-idle-muse: a request typed into an idle Muse never earns its delivered receipt'
status: closed
priority: 0
issue_type: bug
created_at: 2026-10-29T21:11:34.182686353+00:00
updated_at: 2026-10-29T21:16:08.355194504+00:00
closed_at: 2026-10-29T21:16:08.355194393+00:00
---

# Description

Reported 2026-10-29 by the coordinator from another team's bridge (build 945effdb, adopted
Muse coordinator): the second reaction (the delivered receipt) is sometimes missing although
the request is `delivered` and its reply was captured.

## Evidence (read-only, that bridge's state and logs)

| request | ack done (ms) | delivered (ms) | receipt placed |
|---|---|---|---|
| 6ffc01b9 | ...570757 | ...570621 (before ack) | no |
| a530a332 | ...903805 | ...903908 (after ack) | no |
| 0d2751f2 | ...919904 | ...919908 (after ack) | yes |
| e1e1bbd6 | ...969616 | ...969624 (after ack) | yes |

The order of acknowledgement and delivery does not decide it: a530a332 was delivered after
its acknowledgement and still got no receipt. Saving a receipt never waits for the
acknowledgement either (only a disabled acknowledgement skips it), and receipts have their own
worker lane. The receipt directory held no saved entry, and the helper log shows no attempt,
so none was ever saved.

## Cause

A receipt is saved only when the drain reports the prompt verified. An idle Muse pane takes the
plain path, whose submission is unconfirmed; the drain then confirms it through the client's
`working` wait and reports `verified: false`. For a Muse pane that wait reads the screen and
returns only once the prompt left the editor for a new user turn, the input queue or the
journal, which is proof of the same strength as the steer path. A working Muse takes the
verified steer path and earns its receipt, so receipts appeared only for requests that reached
a busy Muse.

## Fix

The client says whether its `working` wait proves the prompt on screen (a Muse composer client
does); the drain then counts that delivery as verified, so the bridge saves the receipt.
