---
title: 'catchup-side-agent: a Catch up side panel that reports only what the owner has not seen'
status: open
priority: 0
issue_type: feature
assignee: mu-catchup
labels:
- vibe-talk
created_at: 2026-11-11T15:50:44+00:00
updated_at: 2026-11-11T15:50:44+00:00
---

# Description

[watermelon-full-preview] Owner replies 366/367 (2026-11-11, 10:43 AM ET), fast-track: "I want to use this ASAP so don't block on me. Put up the design but then rush the implementation and get it deployed ASAP for me to try. We'll test the MVP and iterate."

The voice/text agent is "pretty useless" at catching up on 12 h of unread messages: slow, and it restates what the owner already knows or what is system-administration churn. The owner wants a side agent that shares their point of view: it looks at the thread they are in, models what they already know (their own messages, what they have read, what they replied to), skips churn that resolved itself, and reports product progress toward top-level goals plus blockers, briefly.

Stage 1 design: [ai_docs/2026-11-11-catchup-side-agent.md](https://github.com/rrnewton/dev-agent-utils/blob/main/ai_docs/2026-11-11-catchup-side-agent.md). This issue builds that stage as an MVP and iterates on the owner's use; later stages (follow-ups with tools, voice hand-off, logbook) are separate work.

# Design

- **Entry point.** A "Discuss" button in the channel view and in a thread view opens a side panel tied to that channel or thread: a pop-out on desktop, a sheet on phone (owner reply 373: "Let's just call it Discuss"; internal names may stay). The panel lives in a new `web/src` module; `web/voice.js` is not edited (the #624 refactor owns it) and scroll code is not touched (#1251 owns it). If a `voice.js` mount hook turns out to be unavoidable, the exact functions/lines go to the coordinator for clearance before any edit.
- **Highlight.** The button takes a distinct, still-accessible colour when a non-empty side-agent transcript already exists: for a thread, whenever that thread has one; for a channel (All view), only when the last discussion covered messages still inside the current K window. The server stores the newest message id each discussion covered; a channel button highlights only if that id is within the last K messages, and is plain once the discussion has aged out of the window.
- **Context, built deterministically on the server.** A thread contributes the whole thread; a channel contributes the last K messages (default 150, configurable; lean context for speed, owner reply 376). Each message is labelled `[READ]` or `[UNREAD]` from vibe-talk read state, and `[OWNER]` for the owner's own messages. Unread messages are the subject; read and owner messages are context about what the owner already knows. Correctness of the labels and of the unread-id set matters most.
- **Model.** The existing server-side summarizer path (`meta_gateway`, model `muse-spark-1.4-internal`), effort medium, as summarizer kind `side_agent`: read the model id from `summaries.llm.models.side_agent`, falling back to `summaries.llm.model` (per-kind map coordination, owner reply 372; if this slice lands first it ships that key and the map folds it in). The model id is config, never hard-coded.
- **System prompt.** Editable in Settings as a synced setting, shipped with a default that says: summarize only what the owner has not seen; never restate what the owner wrote or already read; drop operational churn and anything superseded later in the thread; lead with product progress toward top-level goals, then decisions the owner needs to make, then blockers; short bullets citing the message for each point.
- **Transcript.** Each catch-up request and answer is stored server-side per channel/thread, so phone and desktop show the same history. Follow-up questions in the panel go to the same agent with the same context plus the transcript (a second landing if needed to ship the one-shot slice first).
- **Deterministic footer.** The server appends, never the model: "This summary covers N unread messages:" plus a jump link per message and **[Mark read] [Mark unread]** buttons calling the existing read-state API with exactly those unread ids. In the first slice if it fits, else immediately after.
- **Latency (owner reply 376).** One model call, no tools, vibe-talk store/cache reads only — never waits on a coding agent. Show "Reading 42 unread of 180…" at once with the real counts; stream the answer into the panel if the gateway allows it, otherwise one-shot (prefetch on open). Every request carries a latency stamp (context-build ms, first-token ms, total ms, input/output tokens), logged server-side and shown as a small debug line in the panel; the ~3 s / ~10 s targets are measurements, never test assertions. Context order (system prompt, labels legend, messages oldest to newest) stays cache-friendly.
- **Slice order.** Smallest working slice first (Discuss button, panel, one-shot summary with footer and latency stamps; shipped default prompt hard-wired), landed and deployed; highlight, the Settings-editable prompt, transcript follow-ups, and streaming follow in slice 2. Server unit tests cover context building, labels, and footer ids; page tests cover the panel; a browser walk covers phone (393x852 touch) and desktop (1280x800). Tests wait on events or conditions, never wall-clock budgets (owner rule 346). Out of scope: cross-thread context, voice hand-off, tools, glossary/labels.

# Acceptance Criteria

- "Discuss" appears in channel and thread views and opens the side panel (desktop pop-out, phone sheet) bound to that channel/thread, with no `web/voice.js` edit and no scroll-code change.
- The Discuss button is highlighted (distinct, accessible colour) exactly when a non-empty transcript exists for the thread, or for a channel only while the last discussion's newest covered id is still inside the K window (plain once aged out); a browser test covers highlighted vs plain for thread and channel, including the aged-out case, at phone and desktop.
- Server context is deterministic: whole thread, or last K (default 150) for a channel, every message labelled `[READ]`/`[UNREAD]` from read state plus `[OWNER]` for the owner's own; unit tests pin the labels and the window.
- The model call uses `meta_gateway` at effort medium with the model id from `summaries.llm.models.side_agent` (fallback `summaries.llm.model`); the exact live-server overlay lines for any new key are reported at landing.
- The system prompt is editable in Settings (synced) with the design-doc default; answers cover only the unseen, in progress/decisions/blockers order, as short bullets citing messages.
- The transcript is stored server-side per channel/thread and is the same on phone and desktop; follow-up questions work in the panel (first slice or the landing right after).
- The server-appended footer states the unread count, links jump to each covered message, and [Mark read]/[Mark unread] call the existing read-state API with exactly the covered ids; unit tests pin the id set.
- The panel shows "Reading N unread of M…" immediately with true counts; page tests and a browser walk at phone (393x852 touch) and desktop (1280x800) pass; the change is gated at owner tier and landed.

# Notes

Owner replies 366/367 (fast-track, owner tier), 372 (per-kind model map: kind `side_agent`), and 373 (call it "Discuss"; highlight when a transcript exists, channel highlight only inside the K window). Design: https://github.com/rrnewton/dev-agent-utils/blob/main/ai_docs/2026-11-11-catchup-side-agent.md. Filed by mu-catchup before implementation; the first slice lands separately so the owner can try the MVP. Do not edit `web/voice.js` (#624 in flight on another host); do not touch scroll code (#1251 owns it).
