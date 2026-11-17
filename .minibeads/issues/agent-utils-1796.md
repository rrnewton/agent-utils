---
title: 'vibe-talk-bare-short-refs-render: bare :thread:/:msg:/:issue: refs render as short links in vibe-talk'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-17T19:51:00.000000000+00:00
updated_at: 2026-11-17T22:00:00.000000000+00:00
closed_at: 2026-11-17T22:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner UI request, thread 141 (2026-11-17),
fast-tracked. Replies sent through the bridge's reply path are already
converted (`<url|:thread:NNN>` links, #658), and vibe-talk renders a
converted thread/message link as 🧵/✉️ plus the number (#1607, #1713).
But text that bypasses that conversion stays a literal `:thread:NNN`:
TRACE posts (the bridge's transcript/trace path, #714/#832), rescued TUI
transcripts, and pasted text. The owner asks why agents say `:thread:123`
instead of the actual emoji, and directs: in our own formatting (vibe-talk)
match bare `:thread:NNN` and display it as a short link; when the thread's
slug is available show it with the short link
(`[:thread:134/my-slug](<link to the thread in vibe-talk> "Opens here")`).

Already shipped and reused, not rebuilt: #1565 resolves bare
`:thread:`/`:email:`/`:issue:` in vibe-talk from the bridge's
`short-refs.json` (`agentctl-short-refs/v1`) with `data-msg` marking and
in-app jumps; #1607 draws them as emoji plus number; #1713 repairs the
bridge-expanded flattened shape. This issue closes the remaining gaps:
the `:msg:` alias, the slug in the drawn label, an `Opens here` title on
the link itself, and bridge-exactness number guards, so a trace post,
a rescued transcript, or pasted text citing a thread or message draws
the same short link a converted reply does.

# Design

Renderer only (`vibe-talk/src/render.rs`, `vibe-talk/src/short_refs.rs`);
the bridge is not changed in this lane (see Notes for the trace/emoji
investigation and the bridge-side recommendation, for coordb to agree).

- `short_ref_tokens` also matches `:msg:NNNN` as a message reference,
  drawn ✉️NNNN with `data-shortref="email"`, so its tap behaves exactly
  as `:email:` does on the page.
- Number exactness mirrors the bridge (`chat_links::render_tagged`):
  a thread number must be written as the bridge writes its alias
  (zero-padded to three digits, never `0`); a message/issue number must
  be plain decimal with no leading zero and never `0`. A token glued to
  a word or a colon before it is no token, as on the bridge. Code spans
  and fences are untouched (comrak nodes, as before), as is text inside
  a link.
- The slug is vibe-talk's own (coordb, 2026-11-17): `short-refs.json`
  gives the thread's root `ts` for `:thread:NNN`, and this server already
  holds that thread's filed name in `ThreadNames::known_name` (its
  `display_name`), so the renderer joins them at draw time — no bridge
  change, no new file. When the held name is a valid thread slug
  (`summarize::threads::valid_thread_slug`), the drawn label is
  `🧵NNN/slug` — the 🧵NNN form of #1607 plus the slug, chosen over the
  written `:thread:NNN/slug` form and used everywhere a resolved thread
  reference is drawn, including a bridge-expanded link whose label is
  the whole written reference. An absent or invalid name draws plain
  `🧵NNN`. The join is installed like `pr_names` (a memory lookup that
  never waits, queueing nothing), and its stamp joins the render
  cache's key beside the PR-names stamp, so a name filed later is drawn
  on the next read.
- A resolved thread/message short link carries `title="Opens here"`
  (the owner's form; the same words the page already sets on in-app
  links, and no address, per #1565 Q2). Issue links keep their written
  label and no title: they open the tracker, not here.
- An unresolvable number stays unlinked under the existing
  `shortref-unknown` dotted marker, never a dead link. Search render
  version bumps so indexed drawn text is rebuilt.

# Acceptance Criteria

- Unit tests (red before, green after): `:msg:1847` draws as a marked
  ✉️ message link identical to `:email:1847`; a slugged thread entry
  draws `🧵060/<slug>` with `title="Opens here"`; message links carry
  `title="Opens here"` and issue links do not; non-canonical numbers
  (`:thread:60`, `:email:01847`, `:thread:000`) and word/colonglued
  tokens stay literal; `:msg:`/slug forms in code stay text; a
  trace-post fixture (long body with fences, file names, bare refs
  inside and outside code) links only the outside refs.
- Page tests at phone (412x915) and desktop (1280x800), light and dark:
  the slugged thread link drills into the thread, the `:msg:` link
  highlights its message, the unknown ref is no link.
- Screenshot matrix on a test instance (phone + desktop, light + dark,
  keyboard open/closed, Discuss docked), including a trace-post fixture
  and a slug example, looked at image by image and reported to coordb;
  the liaison reviews before anything is announced.
- Investigation reported to coordb: why trace posts leave `:thread:NNN`
  literal, what an actual 🧵 emoji does on the bridge/shim paths, and a
  recommendation on expanding refs on the bridge's trace path. No bridge
  change in this lane.

# Notes

[watermelon-full-preview] mu-refrender, 2026-11-17. Investigation (code
read, this tree): replies expand refs through
`chat_runtime::render_outbound_links_as` → `chat_links::render_reply_links`
(#658/#1709) at send time only. Trace posts never pass through it: the
#714 collector (`chat_trace::Collector::flush`) applies only
`escape_for_chat` (#832: file names into code spans, relative links
flattened) and posts via the tagged path, so a `:thread:NNN` in trace
text reaches Slack exactly as written, and Slack shows it literally
(`:thread:` is an emoji shortcode only with a closing colon; followed by
digits there is none). Rescued TUI transcripts and pasted text bypass
the bridge entirely. The actual 🧵 emoji is safe everywhere checked:
the bridge passes Unicode through untouched (its own tests hold 🧵/🎉
bodies), the wire is UTF-8 JSON, and vibe-talk draws the character as
text — but a bare `🧵060` is not a citation on any path: nothing links
it, and Slack gives it no permalink. Recommendation sent to coordb:
expand refs on the bridge's trace path with the same
`render_reply_links` (stored text stays as written, as replies do);
not built here.

## Resolution

[watermelon-full-preview] Landed 587f4321 on main (gate pass on 87b06c01,
full, 2026-11-17T21:39:33Z; combined tree with the upstream
`vibe-talk/src/main.rs` doctor change verified before landing). The
renderer now draws `:msg:` as a message link, holds bare references to
the bridge's canonical-number exactness, titles thread/message short
links "Opens here", and joins a resolved thread to this server's filed
name as 🧵NNN/slug on page draws (never the index). Unit tests red
before and green after; full vibe-talk lib suite green; browser walk at
desktop and phone sizes green; screenshot matrix (phone 412x915 and
desktop, light and dark, keyboard open and closed, Discuss docked, a
trace-post fixture and the slug example, before and after) reviewed by
coordb's liaison. Not covered: a real-server timeline screenshot (the
fake-Discord backend implements no timeline read) and a live bridge
short-refs file (the resolution path is unchanged from #1565). Bridge
trace-path expansion is #1801 agentctl-trace-posts-expand-short-refs.
