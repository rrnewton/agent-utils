---
title: 'vibe-talk-doc-reader-navigator-campaign: the document reader navigator shows real sections, goes where clicked, and keeps a fixed size'
status: closed
priority: 0
issue_type: bug
assignee: mu-docnav
labels:
- vibe-talk
- documents
- reader
- ui
created_at: 2026-11-25T17:52:21-05:00
updated_at: 2026-11-26T04:47:18.000000000+00:00
closed_at: 2026-11-26T04:47:18.000000000+00:00
---

# Description

Owner request 376 (2026-11-25 5:46 PM ET), verbatim: "When we are actually READING a document in document view... I see some kind of two level navigation bar... but it scuks ... these don't actually represent SECTIONS... i.e. sec 1 sec 2 sec 3.  They should use that section symbol somewhere so it's clean what these are §.  They SHOULD be sections 1 2 1.2 2.3 etc.  But SO MUCH IS BROKEN... did this get through testing? If I click the lower bar it doesn't actually GO to the clicked section consistently.  Clicking left/rigt arrows navigate  the lower bar plus/minus 1.  So that's something.  The top bar doesn't represent sections and doesn't actually GO THERE when clicked.  The whole bar is unstable and changes sizes based on the number of chunks in the bottom.  It should be fixed size and just stretch the chunks. TBH it should be resizable in the width as well as having the grab bar to move around in desktop.  We should drop the numbers from the bottom line that just represents raw chunks.  We didn't used to have them.  Numbers in the top bar should be ACTUAL SECTIONS.  So much is broken and unreliable that I cannot tell how many bugs there are.  If I click the document itself it jumps somewhere... but not where I click.  Maybe offset by some substantial amount from where I click.  WTF are the fat arrows supposed to do!?!?  I thought they were supposed to be SECTIONS (top bar) and the thin arrows control chunks i.e. the lower bar.  You need a big fix and test campaign here."

This is the document reader's read-aloud navigator: the floating panel with two rows of chunk cells, fat and thin step arrows, and the position counter. The owner reports that neither row shows real sections, clicks on either row and clicks in the document body land somewhere other than where clicked, the panel changes size with the chunk count, and the two arrow pairs' jobs are not discoverable. The campaign first diagnoses each behaviour on a local server (root cause, file and function, and an audit of why existing coverage caught none of it), then designs, then lands small gated fixes with real-browser tests and a recorded walk-through.

# Design

Approved after diagnosis and liaison review (2026-11-25). Diagnosis summary: navigator and body taps sent a page block-model index that the cloud read path consumed as a server prepared-block index; the server cuts blocks into spoken pieces (one per table body row, long blocks into sentence pieces), so taps after any table or long block landed early on the default (cloud) voice, while device-voice taps and the segment-space arrows stayed exact. Reproduced on a local server (canary with a long paragraph and a table before the targets: section and body clicks lit the table on cloud, exact on device and on a no-trigger control). The one navigator browser test forced the device voice over short single-piece paragraphs with locator-centre taps, so it could not see this.

- One index space: every document position between reader and read-aloud owner is in page space (model block plus offset/piece within it). Mapping to server pieces happens only inside the owner, by element groups (the grouping position reporting already uses); no stored conversion. Out-of-range clamps; unaligned cuts fall back by server drawn element, never a silent restart at the top. Exact on both voices (device speech derives its list 1:1 from model texts).
- Body taps resolve the tapped word by coordinates (caret from point; table taps resolve the row). While playing: jump there, no scroll, no whole-block centring. While paused: move the cued position and stay paused. With no read playing: set a read-start marker at the tapped line and do not speak (links and text selection keep working); Read starts at the marker. Every tap paints its feedback in the same frame. Same-document taps jump on the retained read (no fresh prepare, pause preserved).
- Sections row (top): every real section from the heading tree as "§ 1.2 Title", flat in document order, no folding and no gap cells; the row scrolls when long. Titles truncated but visible on desktop; on phone, section numbers with the title on tooltip/long-press. Author numbers never duplicated. A document with no headings shows a single "§ Whole document".
- Chunks row (bottom): unnumbered ticks for the current section's blocks, stretching to the fixed panel width (weight shares, 8 px minimum, then the row scrolls). Clicking a tick lands on that block.
- Arrows: fat steps sections of the same heading tree the top row shows; thin steps visible chunks (one model block per press, every press changes the lit chunk). The position count for documents counts model blocks. Accessible names say section/chunk.
- Fixed geometry: while a document read stands, both rows always render at fixed heights and the desktop panel width is the chosen width (default custom property), never content-sized; cell budgets derive from the chosen width. Desktop width is resizable by an edge drag and persisted per device beside the existing drag offset. On phone the docked strip replaces the bottom toolbar while a read stands (no overlap), full width, same fixed rows.
- Rendering: cells built once per document (chunks per current section); position reports only move current/heard marking and scroll the current cell into view — no per-block rebuild, one delegated click listener, focus survives block boundaries.
- Landing order: (A) the index fix alone with a cloud-voice canary test and the existing scrubber walks on both voices; (B) sections row, ticks, one section definition, no-rebuild rendering; (C) fixed geometry and resize; (D) body-tap modes and same-document jumps. Each step gated separately with dynamic local-server evidence. Deferred: client/server block-cut divergence classes beyond the aligned-cut mapping (fallback semantics preserved).

Related work already landed: #2242 vibe-talk-documents-reader-toolbar (the reader controls docked in the bottom toolbar) and #2241 vibe-talk-document-load-502-readable (readable document load failures with Retry). Their commits and files are checked for overlap before any edit, as are the other reader-page changes in flight.

# Acceptance Criteria

- Diagnosis first: every behaviour the owner names (and any further defect found) is reproduced on a local server from the worktree build, with its root cause, file and function, plus an audit of existing test coverage explaining why none of it was caught. No code before that report.
- The top row shows real sections numbered as headings with the section sign; clicking a section lands the reader and read-aloud at that section's start.
- The bottom row shows raw chunks with no numbers; clicking a chunk lands on that chunk.
- The fat arrows move by section and the thin arrows by chunk, with labels, tooltips and accessible names that say so.
- The panel keeps a fixed size whatever the chunk count (cells stretch); on desktop its width is resizable and it can be dragged by a grab bar, with size and position persisted per device.
- A click in the document body lands where clicked.
- Real-browser tests click each control, each row's cells and in-body points and assert the landing (the visible section heading, the chunk index and the read-aloud position), at phone and desktop sizes, on several fixture documents (flat headings, nested headings, long and short), through the real server routes. Condition-based waits only; a browser test is committed only when cheap, fast and high-signal, and every fix is exercised dynamically on a local test instance with the evidence attached.
- Screenshot matrix on a local test instance (light and dark, keyboard open and closed, phone 412x915 and desktop 1280x800, Discuss open in the phone shots) plus the navigator states before and after, each folder with its build record, and a recorded walk-through for review before landing. Fixes land in separate small gated steps where possible.

# Notes

Owner-UI fast-track (owner-priority gate, deployed on landing). Reproduce and test only on a local server from the worktree build with fixture data; never the live instance.

Step A (one page index space for document reads: navigator and body taps seek in the page's block space on both voices; the device voice speaks the page's own texts) landed as caf7a08b on a green owner gate on the same tree (e5dd9542, 2026-11-25 9:02 PM ET) whose selection ran the scrubber, read, control-bar and read-aloud-blocks browser checks; deployed the same evening with a clean 15-minute watch. Steps B, C and D follow.

## Resolution

[watermelon-full-preview] Landed on github/main in four gated steps, each deployed on landing: step A `caf7a08b` (one page index space for document reads — navigator and body taps seek in the page's block space on both voices, mapped to spoken pieces by element groups inside the read owner; the device voice speaks the page's own texts), step B `d3d15491` (every section flat as § number and bare title, unnumbered chunk ticks, cells built once per document/section behind one delegated listener, fat arrows stepping the payload's section tree, thin arrows one model block, the strip count in model blocks), step C `7b96b335` (the desk strip at the reader's chosen fixed width by its right-edge handle, remembered per device; fixed row heights), step D `0ab352c1` (deployed 11:45 PM ET, watch clean; body taps resolve the tapped word or table row — playing jumps there on the retained entry without scrolling, paused cues held, no read marks where Read starts and says nothing; clipped row edges fade and reveal by wheel, drag and arrow keys). Diagnosis root cause for the wrong landings: taps sent a page block-model index the cloud path consumed as a server prepared-piece index (tables speak per row, long blocks per sentence piece), so landings after any table or long block started early on the default voice while device taps and segment-space arrows stayed exact; the one navigator browser test forced the device voice over short single-piece paragraphs and could not see it. Tests: a cloud-voice canary in the scrubber (fails on the old seek, with the existing walks on both voices), pure mapping round-trips, marker/held/offset tests in the fake page and owner suites, geometry and taps walks in the scrubber, and screenshot scenes 132-137 with the final matrix (light and dark, phone 412x915 and desktop 1280x800, Discuss in the phone shots) taken from the landed build. Deferred, noted in step A's evidence: client/server block-cut divergence classes beyond the aligned-cut mapping keep the drawn-element fallback a concrete document can still exercise; no issue filed yet at the coordinator's direction.
