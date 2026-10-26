# documents-view: a third major view for the documents agents link in chat (design)

[opus 5.5] Design and planning only, written 2026-10-26 for the owner to read before anything is
built. Nothing here is implemented. `#544 documents-view-design`.

## The ask

The owner, 2026-10-26 (condensed). Agents often answer with a report committed to GitHub, or posted
to an internal paste service, and reply with a link. Both are web pages showing rendered Markdown,
with a way to get the raw source. Wanted: a convenient view, readable in the car, listing the linked
documents in time order. Tapping one opens it full screen, to read it rendered in the app or to
press Read and hear it, the way long chat messages already work. Something like the thread list:
a small card per document, and a tap gives a full-screen rendering of just that document. These
documents tend to be long. Can we recognize their URLs and fetch them to build that list? It becomes
a third major view beside the voice view and the channel view. In the short term the two-way toggle
becomes a button that opens a picker of the three. Agent apps already do this with an artifacts tab.
We do it the disaggregated way: hyperlinks to ordinary websites and bridges to several chat
services, not one platform or one agent. Make proposals and recommendations, call out the hard
parts, and say what we can do with the raw Markdown, given the tension between server-side and
client-side rendering.

## Answers first

1. **It is feasible, and many of the parts exist.**
   - The server already renders GitHub-flavoured Markdown safely (`render.rs`: comrak,
     highlighting, tables, ammonia as the boundary).
   - It parses GitHub addresses strictly (`link_labels.rs`).
   - It prepares text for speech (`speakable.rs`).
   - The message cache already indexes the text of every message, link addresses included, across
     channels.
   - The in-flight `#543 in-app-message-links` lane is building the mechanism a document link
     needs: the renderer marks a link with what it names, and the page turns the tap into an
     in-app jump that Back undoes.

   What is new:
   - a recognizer and a fetcher with credentials;
   - a second, document render profile with its own sanitizer settings;
   - speech sections cut from the parsed tree;
   - endpoints, and a pane.
2. **Recognizing the links is easy and reliable for two kinds.** Over 12 days of the owner's Slack
   channels, the document links were almost all of two shapes: GitHub `blob/…/*.md` pages (50
   documents) and internal paste pages (75 pastes). Both yielded raw Markdown every time we tried:
   **125 of 125** (§ Findings). Google Docs appeared once.
3. **The hard parts are not rendering.** Rendering 300 KB takes about 10 ms. The hard parts are
   these (§ The challenging issues):
   - whose credentials fetch a private document, and who may then see it;
   - documents change after they are linked: 104 of 106 GitHub blob links name `main`, and 17 of 50
     documents changed afterwards;
   - relative links: 32 of 50 documents link to sibling files, 544 links to other Markdown files;
   - length: the median is about 4,000 words, roughly 27 minutes aloud;
   - read-aloud that survives a locked phone in a car.
4. **Recommendation: start with design A+, then B, then C.**
   - **A+ (the MVP):** the documents list is built on demand from the message cache, which is
     already cross-channel and already filled only by the owner's reads. Fetched documents are
     kept in a bounded cache, not a new durable index. The MVP is the three-way switcher, GitHub
     Markdown, the list, the reader and Read by section.
   - **B (a durable document index):** only when a deployment needs documents without a message
     cache, or versions that survive restarts.
   - **C (logbook):** when that storage layer exists.

   Render on the server, cache the result per version, and send the page sanitized HTML and an
   outline. The MVP is about two weeks of lane time (§ Phased plan), more than the switcher alone
   suggests.

## Findings

All numbers are from 2026-10-26. They were measured on a copy of one deployment's message cache: the
owner's Slack instance, 1,784 messages over four channels, 2026-10-14 to 2026-10-26. The live file
was never opened. The Google Chat instance keeps no message cache and was not sampled. Repository
names and channels are redacted here. The unredacted numbers are in the owner's private notes.

### Which document links appear

"Mentions" count each distinct address once per message.

| Kind | Mentions | Unique | Raw Markdown obtainable |
|---|---:|---:|---|
| Internal code-review pages | 446 | 121 | not documents; out of scope |
| **Internal paste pages** (two URL shapes for one id) | 97 | **75 pastes** | **75 / 75**, via the service's CLI as the owner, ~1 s each |
| **GitHub `blob/REF/…/*.md`** | 103 | **50 documents** | **50 / 50**, via the GitHub API with the owner's full `gh` token |
| GitHub `blob/…` other files | 3 | 3 | not documents |
| GitHub issues / pull requests / commits | 204 | 124 | not documents (issue bodies could be, later) |
| GitHub `tree/…` (directories) | 7 | 5 | no: a directory, not a document |
| GitHub Pages site | 4 | 1 | HTML, not Markdown |
| Google Docs document | 4 | 1 | needs Google OAuth; not tried |
| Gists, `raw.githubusercontent.com` | 0 | 0 | none seen |
| Source file names that Slack turns into links (`http://validate.rs`) | ~37 | 23 "hosts" | false positives (see below) |

So **125 documents in 12 days**, about ten a day, and rising: 41 messages with a document link on
the busiest day. 153 messages carried one, 117 of them thread replies, and 126 carried exactly one
document. 99 of the 103 GitHub Markdown mentions were posted by bots, that is, by agents.

The 50 of 50 was fetched with the owner's own `gh` token, which reads every repository the owner
can. The least-privilege token recommended below reads only the repositories it is given. Coverage
then depends on that list.

### What the documents are like

| | GitHub Markdown (50) | Pastes (75) |
|---|---|---|
| Size p50 / p90 / max | 29 KB / 175 KB / 317 KB | 6.4 KB / 21 KB / 44 KB |
| Over the chat renderer's 16 KiB bound (`MAX_RENDERED_BYTES`) | **38 / 50** | 16 / 75 |
| Words p50 / p90 / max | 4.0 k / 19.6 k / 37 k | not measured |
| First line is a `#` or `##` heading | 44 / 50 | 72 / 75 |
| Headings (H1–H3), median per document | 12 | not measured |
| Has relative links | **32 / 50** (717 links: 544 to `.md`, 109 to `.json`) | n/a |
| Has images | 1 / 50 (4, all relative) | not measured |
| Has tables | **45 / 50** (336 tables) | common |
| Has code blocks | 21 / 50 (60 blocks) | not measured |
| Raw HTML, Mermaid, math | 1, 1, 0 documents | not measured |

Other facts that shape the design:

- **Links point at a branch, not a version.** Of the 106 GitHub blob links (the 103 Markdown
  mentions and 3 to other files), 104 name `main` and 2 name a commit. **17 of the 50 documents
  had commits after they were first linked**, between 1 and 29 each. A paste is a fixed snapshot;
  editing one makes a new paste.
- **The same document is linked again and again.** 24 of the 50 GitHub documents were linked more
  than once, one of them eight times. A list of links would be a list of repeats.
- **Links carry labels.** Slack sends `<url|label>`, which the reader turns into `[label](url)`.
  Counting every occurrence, repeats within a message included, 198 document links had a label and
  4 were bare. When a document has no heading, the label is the next best title.
- **Slack turns file names into links.** An agent writing `validate.rs` in prose reaches the cache
  as a link to `http://validate.rs`. These made about 37 link mentions in 12 days. A recognizer
  that matches only known hosts and routes ignores them for free. A heuristic like "any link
  ending in `.md`" would not. `.md` is a country domain like `.rs`, so `README.md` in prose can
  arrive the same way. There were none in this sample.
- **One GraphQL request checked all 50 documents.** It returned each one's current blob id and
  size. So a refresh of every recent document costs one API call, not fifty.
- **The paste CLI returns content only**, with no title, author or time. Titles come from the first
  heading, which 72 of 75 pastes have.
- **Rendering is cheap; highlighting is the variable.** comrak 0.55 plus ammonia 4.2.1 on this host,
  without syntax highlighting, best of five runs:

  | Input | Time |
  |---:|---:|
  | 16 KiB | 0.3 ms |
  | 64 KiB | 1.6 ms |
  | 300 KiB | 10 ms |
  | 844 KB (vibe-talk's README) | 27 ms |

  Highlighting was not measured. The chat renderer caps it at 80 ms per message
  (`HIGHLIGHT_BUDGET`) and draws the remaining blocks plain. A code-heavy document needs a budget
  of its own. Done once per version and cached, neither cost matters to a reader.

## What exists to build on

| Need | Already in vibe-talk | Gap |
|---|---|---|
| Recognize a document address | `link_labels.rs`: a host and route table with strict, never-decoded parsing. The renderer consults it only for a link whose text is its own address, and 198 of 202 document links carry a label. | a `documents` route set, run on every link and on the source text itself, and a canonical document key |
| Mark a link so the page can take the tap | `#543 in-app-message-links` (in flight): the renderer marks message permalinks with an attribute, and the page decides whether to make the tap a jump | the same, for document links |
| Find every document link across channels | the message cache: every message's text with a trigram full-text index, filled only by write-scope and server-owned reads | a query over it (design A+) |
| Render Markdown safely | `render.rs` pipeline; the ammonia allowlist is the security boundary; server highlighting (`#531`) | a document profile with its own sanitizer settings (listed below) |
| Read text aloud | `speakable.rs`, a line-by-line stripper over raw text (code blocks over three lines named, tables announced, addresses said as pages, times made relative); browser and server read-aloud backends; the read strip with Pace and prev/next | sections cut from the parsed tree (new code), a resume position, background audio |
| Jump with Back | `#423 back-key-consistency` places, `#304` history entries, `#462 webapp-deep-links` (`?v=…&m=…`) | a `docs` view and a `d=` parameter |
| A list of cards | the thread list (`#279`, `#407`) | a document card |
| A local copy on the phone | the saved-messages snapshot (`localStorage`, about 600,000 characters in total) | too small for documents: use IndexedDB |

## Alternative designs

A+, B and C serve the same page API, so the backing can change without the page noticing:

- `GET /api/v1/documents?before=&limit=&channel=` returns cards.
- `GET /api/v1/documents/{key}` returns metadata, HTML, the outline, mentions, and "changed since
  linked".
- `GET /api/v1/documents/{key}/speech?section=N` returns speakable chunks.

### A. A thin reader on what the page already holds

The page builds the list from the messages it has loaded, as the Links filter does today. Opening a
document calls a fetch-and-render endpoint.

- **Cost:** the smallest. The switcher, a list, the reader, and one endpoint.
- **For:** it ships fastest and adds no state.
- **Against:**
  - The list holds only what the page has loaded: a window of the current channel, read in pages
    of 50, and none of the other channels. The owner's question is "what did my agents produce
    this week", and A cannot answer it.
  - No dedupe across channels.
  - Every phone re-derives the list.

### A+. The list from the message cache, documents in a bounded cache (recommended MVP)

The server answers the list endpoint by querying the message cache:

1. The trigram index finds rows holding a document host.
2. `documents::recognize` parses each address in the text, not the rendered HTML, so a body over
   16 KiB, which the renderer draws as plain text, is still read.
3. Rows are grouped by document key, newest mention first.

Fetched documents (raw source, rendered HTML, outline, version id, ETag) live in a cache bounded by
bytes, which a restart may empty. A conditional request revalidates them cheaply.

- **Cost:** medium. See Phase 1.
- **For:**
  - It answers the real question, across channels, with dedupe and titles.
  - It adds no new durable index. The message cache's existing rules hold: only the owner's reads
    fill it; a tombstone or an aged-out row removes the mention; the owner's off switch and the
    storage purge erase it.
  - One fetch serves every phone.
- **Against:**
  - It gives nothing where the message cache is off. That is the Google Chat deployment today.
  - Versions and resume positions keyed by version need the GitHub history on demand, or the page's
    own memory.
  - The list query does work proportional to the candidate rows, cached briefly.

### B. A durable document index

The server records a **mention** for each document link in each message it reads with the write
token or on its own behalf. It fetches each **document** once per version and keeps every version.

```
messages (write-scope and server-owned reads, live pushes)
   │  documents::recognize(address) → DocKey
   ▼
mention(doc_key, provider, channel, message_id, thread_id, posted_ms, author, is_bot, label)
   │
   ▼                                   fetcher (per source kind; owner credentials; allowlisted)
document(doc_key, kind, locator, title, newest_version, first_linked_ms, last_linked_ms)
   │
   ▼
version(doc_key, version_id, fetched_ms, bytes, raw, html, outline, section_structure, etag)
```

- **Storage:** its own SQLite file beside the store, `0600`, bounded by age and size, with its own
  off switch.
- **Erasure, which B must get right and A+ inherits:**
  - A document is erased when its last mention goes: the message was tombstoned or aged out, or
    its channel left the server's list.
  - The storage purge and the off switch erase everything.
  - A document reached only through another document's relative link lives only as long as the
    document that links it.
  - Fetched content never outlives every message that justified fetching it.
- **No read-token path.** A read with the read token writes nothing, as with the message cache. So
  a voice agent's read can neither add mentions nor cause a fetch with the owner's credentials.
- **Cost:** A+ plus about a week: the store, its lifecycle and erasure, the backfill, migrations,
  and tests on the scale of the message cache's own.
- **For:**
  - It works without a message cache.
  - It keeps versions across restarts.
  - It is a clean projection of messages, ready to move into logbook.
- **Against:** a second durable store of private content, with all the erasure rules that implies.

### C. Documents as a first-class source in logbook

The master plan's logbook (`2026-10-23-vibe-talk-next`) stores facts: versions, tombstones,
listings. Documents become a third source beside chat and transcripts.

- A fetched version is a `Version` fact.
- Summaries key on content versions, so each card gets a one-paragraph abstract through the summary
  ladder.
- Research and the voice agent read documents through a `documents` capability.
- The Work tab joins documents to the agents that wrote them.

- **Cost:** large, and gated on logbook (0.3.0 and later in that plan).
- **For:** the most capable end state, with one data model and one sync story.
- **Against:** it cannot start now. Most of its value over B (abstracts, agent access) is exactly
  the part that needs the access-control answers first.

### Rendering: server or page?

The question was the tension between server-side and client-side Markdown rendering. **Render on
the server, once per version, and cache it.**

- **One renderer for chat and documents.** The same comrak, the same highlighting, the same table
  styles. A second Markdown renderer in the page would drift from the first. The page stopped
  hand-rolling Markdown for exactly that reason (`#217 markdown-blocks`).
- **One sanitizer library, two configurations.** A document is text from anyone who can push to a
  repository the owner can read, so ammonia stays the boundary. Documents need attributes chat
  forbids (below), so they get their own allowlist. That allowlist gets its own tests, not a
  loosening of chat's.
- **Structure comes from the parsed tree.** The outline, the sections and the code blocks are read
  from comrak's tree once, on the server.
- **Render time is not an argument either way.** About 10 ms per 300 KB version before
  highlighting, once.

What the page still needs is the HTML at rest for offline reading, and a light touch when drawing.
It caches the sanitized HTML and outline in IndexedDB, and draws long documents section by section
with `content-visibility: auto`, so a 350 KB document does not lay out all at once on a phone. A
client renderer would be warranted only for a fully offline page with no server, which vibe-talk is
not.

The **document render profile** differs from the chat profile:

1. **No provider dialect.** A document is plain GFM. There is no Slack or Google Chat link syntax,
   no mention chips, no one-asterisk bold, and no `•` bullet repair. Setext headings are parsed,
   as GitHub does; chat sets `ignore_setext`.
2. **Bounds.** A cap of 1 MiB of source instead of 16 KiB, with the table-cell, depth and growth
   bounds scaled to match, and a highlighting budget per document instead of 80 ms per message.
   Past the cap, show the source as text with "Open original".
3. **Heading anchors.** comrak's `header_id_prefix` gives every heading an id with a fixed `doc-`
   prefix. The document's own `#fragment` links are rewritten to match. Chat's sanitizer allows
   no `id` at all and strips relative and fragment-only addresses (`UrlRelative::Deny`). The
   document allowlist admits `id` only on headings and only with the prefix, and fragment links
   only to those ids. See the DOM issue for why the prefix is not optional.
4. **Links.** Chat opens every link in a new tab (`target="_blank"`). In a document, an in-document
   anchor scrolls the reader, a link to another document opens it in the reader, and only other
   links leave the app.
5. **Relative links** are resolved against the document's own repository, ref and directory:
   - a link to another `.md` becomes an in-app document link (opened in the reader, with Back);
   - any other file becomes its GitHub page;
   - a `#fragment` becomes the prefixed anchor.

   Any `..` that would leave the repository is left unresolved, as `link_labels.rs` already refuses
   dot segments.
6. **Images** become links, as in chat, for the MVP. Private-repo images cannot be fetched by the
   phone without credentials, so showing them later means a server proxy with the same allowlist.
   That is worth doing only if the owner's documents start to carry figures; 1 of 50 does today.
7. **Front matter** is dropped from the body, and its `title:` is used as the title.

## Recommendation

**Build A+ first.** Its MVP covers the GitHub half of the owner's documents and the whole UI.
Pastes follow as the second source. Build B only when a deployment needs documents without a
message cache, or durable versions. Keep the document key and the page API stable, so A+ → B → C is
a change of backing, not of the page.

Specific choices:

- **The document key** is canonical:
  - `github:OWNER/REPO:PATH` for a link to the default branch, so one card stands for every such
    link;
  - `github:OWNER/REPO@REF:PATH` for any other branch;
  - the commit for a link pinned to one;
  - `paste:ID` for a paste, whose two address shapes give one key.

  Splitting `blob/REF/PATH` is not syntactic: a branch name may hold a slash, and `link_labels.rs`
  declines to guess. The fetcher resolves the longest prefix that is a real ref through the API,
  once per address, and caches the answer. A link to a branch that is later archived names a ref
  that no longer exists. The card says so, and offers the copy fetched while the branch existed.
- **Show the newest version, and say when it changed.** The card and the reader show
  "updated 2 h after it was linked". For a branch link, the reader offers "as linked": the version
  current when the message was posted, found from the commit history up to that time. Agents keep
  editing reports after they link them, and the newest version is usually what the owner wants.
  Question 2 asks.
- **Least-privilege GitHub credentials.** A fine-grained token with Contents: read on chosen
  repositories, read from a secret file. It is not the `gh` CLI's own OAuth token, which can read
  every repository the owner can. Use conditional requests (a `304 Not Modified` does not count
  against GitHub's rate limit) and one batched GraphQL query for refresh.
- **Owner-only access, and no model sees a document in the MVP.** See the access-control issue.
- **The paste source is a configured command, not code that names a service.**
  - An operator adds a source with a URL pattern and an argv template. The template must put `--`
    before the placeholder.
  - The captured id must match `[A-Za-z0-9][A-Za-z0-9_-]{0,63}`. A leading `-` is refused, so an
    id can never become an option.
  - The server runs the command directly, never through a shell, with a timeout, a size cap and a
    concurrency of one. Its stdout is the raw Markdown.

  That keeps internal services out of this public repository, and serves any other paste-like
  service the same way.
- **One list across every channel on the server's list, newest mention first,** with a channel
  filter chip. A card says "linked 3×". The owner asks "what did my agents produce", not "what was
  posted here".

## UI

### Three major views, and the interim switcher

Today `#view-switch` is a two-state switch (`aria-checked`, labelled Voice or Channel). In the
interim it becomes a **button showing the current view's name**, which opens a three-item menu:
**Voice**, **Channel**, **Documents**. The menu is built like the existing pickers, as a list of
radio items with the current one checked. It needs no new layout, and fits whichever
`channel-bars-layout` option the owner picks later: there it becomes one of the major-view tabs.

- **Keyboard:** Ctrl+0 is the call's view (`#525`) and Ctrl+1 to Ctrl+4 are the channel's views
  (`#281`). Ctrl+5 for Documents is the natural next chord. The README says Chromium hands Ctrl+1
  to Ctrl+8 to the page. Ask before taking it (question 7).
- **Deep links:** `?v=docs` opens the list and `?v=docs&d=<key>` opens a document (percent-encoded
  key, never the fragment, as `#462` requires). The `#423` places gain "documents list" and
  "document D at section S".

### The list

The list reuses the thread list's card styles:

```
┌──────────────────────────────────────────────────────────┐
│ Replication model: what the page, the server and …      │  ← first heading, else link label, else file name
│ repo-a:ai_docs/…/replication-model.md  · paste           │  ← source, as link_labels would label it
│ agent-name · #channel › thread name · 3h · linked 3×     │  ← newest mention
│ 4,100 words · ~27 min · updated 1h after linked     ● 40%│  ← reading time, version note, read progress
└──────────────────────────────────────────────────────────┘
```

- **Filters:** search over titles and sources, a channel chip, and "unread". Unread means never
  opened. Partly read shows the progress.
- **Not fetched yet** (credentials missing, a fetch failed, a host with no fetcher): the card stays,
  titled by its link label, with the reason in words, and tapping it opens the original. A failure
  is visible, not dropped, as with failed summaries.
- **The Links filter in the channel view stays.** It answers "what was linked here". The documents
  view answers "what documents exist".

### The reader

- **Full screen.** A slim header holds Back, the title, the source chip, **Read**, and **⋯**. The
  menu offers: Open original, Copy link, View raw, Linked in…, and As linked / Newest.
- **The body** is the document's sanitized HTML in the page's reading width and text size settings.
  Wide tables scroll sideways inside their own box (45 of 50 documents have tables). Code blocks
  wrap or scroll as in chat.
- **Contents** is a drawer from the header, built from the outline. Tapping a heading jumps there.
- **Positions are anchors, not pixels.** The reader's place, the resume point and follow-along are
  kept as section and paragraph indexes. `content-visibility: auto` makes pixel offsets shift as
  sections are laid out, so a saved `scrollTop` would land somewhere else.
- **Moving between documents and messages:**
  - A document link in a message opens the reader in-app, and Back returns to the exact message.
    This is `#543`'s mechanism with a second attribute, so the setting that keeps message links
    in-app covers both.
  - **Linked in…** lists every message that linked the document. Choosing one jumps there by the
    `#462` message link, and Back returns to the reader at the same place.
  - A relative link to another `.md` in the same repository opens that document in the reader,
    with Back. 544 such links in 50 documents suggests the owner will use this.

### Read-aloud for long documents

The median document is about 27 minutes aloud, and the 90th percentile over two hours. The
read-aloud has to be navigable, not one long utterance.

- **Sections.** The server cuts the parsed tree at headings into sections, and sections into
  paragraph chunks. That is new code: `speakable.rs` works on raw text, line by line, and knows only
  backtick fences. The cut structure is cached per version.
- **Speech is prepared when it is spoken, not cached.** Each chunk goes through `speakable.rs` at
  speak time:
  - code blocks over the short threshold are named rather than read;
  - tables are announced by their keys;
  - addresses are said as pages.

  It cannot be cached, for two reasons. `speakable.rs` makes times relative to now ("three hours
  ago"). And it names hashes from a table shared across the server (at most 4,096 entries), so
  the letters must belong to the listener's session. Naming every fetched document in advance would
  also fill that table.
- **Controls** are the read strip's own, plus:
  - **Next / Previous section**, beside Pace;
  - tap a paragraph to read from there;
  - a "Skip code" setting, on by default.
- **Resume.** A resume position (section and paragraph) is saved per document version, and offered
  when the document is reopened.
- **Follow along.** The paragraph being read is highlighted and kept in view, as the row being read
  is today.
- **In the car**, the screen is off or another app is in front. Web Speech, the device voice, is not
  guaranteed to keep speaking then (the README already says so). Durable background playback needs
  audio in an `<audio>` element with the Media Session API:
  - Android then keeps playing with the screen locked and shows media controls;
  - a car's steering-wheel "next track" can map to **next section**.

  That means server-side synthesis of each chunk, which raises the question of which voice service
  may receive a private document (question 3). The MVP uses the device voice with the screen on.
  Phase 3 adds the audio path.

## The challenging issues

1. **A confused deputy with the owner's credentials.** Anyone who can post in a watched channel
   can link any document the owner's token can read. The server will fetch it as the owner. That
   is harmless only if everything that can see the result is the owner's.
   - The read token is the one pasted into the voice agent, which a third party hosts. So
     **document endpoints must not accept the read token**. They require the write scope, or a new
     documents scope. The cost: a phone paired with only the read token sees no documents.
   - **Nothing reachable with the read token may cause a fetch** or record a mention. A+ gets this
     from the message cache, which read-token reads never fill. B must keep the same rule.
   - The voice agent gets no document tool until the owner decides otherwise.
   - A least-privilege GitHub token narrows what a stranger's link can reach.
2. **Where private text goes when it is read aloud.** The device voice keeps it on the phone. A
   self-hosted speech stack keeps it on the owner's machines. A commercial voice API receives the
   whole document. Chat messages already take that path when read aloud through a vendor; a
   private design document is a larger disclosure. The default should be device or self-hosted,
   with the vendor as an explicit choice per deployment.
3. **Documents change after they are linked.** 104 of 106 GitHub blob links name a branch, and 34 %
   of the documents changed after they were linked. A card must say which version it shows. A
   refresh must be cheap (conditional requests, batched queries), and "as linked" must be
   recoverable. Branches get archived and deleted, so a link to one can stop resolving. Pastes are
   immutable, which makes them easy.
4. **Relative links and refs.** Relative links are common (32 of 50 documents) and need repository
   context the chat renderer never had. Every resolution must stay inside the repository; a crafted
   `..` path is the classic escape. Where the ref ends and the path begins needs the API, because
   branch names may contain slashes. Private images need a proxy, or stay links.
5. **DOM id clobbering and style leakage.** vibe-talk's page finds its own elements by id
   (`el("view-switch")`, and hundreds more). A document heading "View switch" with plain heading ids
   would create a second `id="view-switch"` in the same document. Heading ids therefore get a
   fixed prefix that no page id uses. That rule is enforced in the document sanitizer's attribute
   filter, not only in the renderer. The alternative is a sandboxed `<iframe srcdoc>`. It isolates
   ids and styles completely, but makes theming, text size, follow-along highlighting and position
   restoring cross-frame problems. Prefer the prefix, and keep the iframe as the fallback.
6. **Completeness of the list.** The list is only as complete as what the server has seen. In A+ it
   is the message cache's window (`max_days`, 90 by default) on the deployments that run one. A
   message over 16 KiB must still be scanned, which is why recognition reads the source text and
   not the rendered body. The view must say what it covers, in words.
7. **Length on a phone.** A 317 KB document renders to about 350 KB of HTML, with tables. Section
   rendering and `content-visibility` keep layout cheap, at the price of pixel positions (see
   the reader). The offline copy needs IndexedDB: the saved-messages `localStorage` budget is about
   600,000 characters in total.
8. **Recognition precision.** Match only known hosts and routes, through the same strict parsing as
   link labels: no percent-decoding, no dot segments. Slack's file-name links (`http://lib.rs`) and
   code spans then never become documents. Directory (`tree`) links are not documents in the MVP.
9. **Fetching safely.** There is no generic "fetch any URL", which would be server-side request
   forgery with the owner's network position. Each source is an allowlisted host with a dedicated
   fetcher. Every fetch has:
   - a size cap (1 MiB) and timeouts;
   - limited concurrency;
   - backoff on GitHub's rate-limit headers.

   A command source runs without a shell, after `--`, on an id that cannot start with `-`.
10. **Text that reaches a model.** Not in the MVP. When documents feed summaries, research or the
    voice agent (design C), they are untrusted third-party text. They go through the same fence as
    chat (`untrusted.rs`). A document saying "ignore your instructions" is a document, not an
    instruction.
11. **Non-Markdown documents.** GitHub Pages sites (HTML), PDFs, Google Docs and code files. The MVP
    lists only Markdown and plain text. A `.json` or source-file link stays a link. Google Docs can
    export Markdown, but only with Google OAuth for the owner's account. One link in 12 days does
    not justify that yet.

## Phased plan

Each phase is shippable on its own and keeps the owner able to say stop. Estimates are lane-days of
one agent, rough, and include tests and browser walks. For scale, the message cache that A+ reads
is about 29,000 lines.

**Phase 1, MVP (A+): switcher, GitHub Markdown, list, reader, Read.** Two paired lanes, about
5–7 lane-days on the server and 5–8 on the page, so one to two weeks elapsed.

- **Server:**
  - `documents::recognize` and the document key for `github.com/OWNER/REPO/blob/REF/PATH.md`
    (and `.markdown`, `raw.githubusercontent.com`), with ref resolution;
  - the list endpoint over the message cache;
  - the GitHub fetcher, with a token from a file, conditional requests and a byte-bounded cache;
  - the document render profile and its sanitizer allowlist: bounds, prefixed anchors, links,
    relative links, outline;
  - section cutting from the tree, and speech per chunk at speak time;
  - the endpoints, owner scope only;
  - tests, including a fixture repository with relative links, a `..` escape attempt, a branch name
    with a slash, a heading that collides with a page id, and a document link in a message over
    16 KiB.
- **Page:**
  - the three-way view picker, and the `docs` view and `d=` deep link;
  - the Documents pane: cards, filters, the "covers …" line;
  - the reader: header, body, contents drawer, Linked in…, anchor positions, Back through `#423`;
  - document links in messages open in-app (on top of `#543`);
  - Read by section with the device voice;
  - browser walks at 393×852 and on a desk.

**Phase 2: pastes, versions, the car copy.** About 3–5 lane-days on each side.

- The **command source**, configured on the internal deployment for the paste service.
- **Versions:** "updated since linked", "as linked" for branch links, and the batched background
  refresh.
- **Offline copy:** the newest N documents, with their HTML and outline in IndexedDB, tied to the
  token like the saved messages. Opening a document saved for the car works with no signal.
- **Resume positions.**

**Phase 3: listening in the car.** About 3–5 lane-days.

- Server-synthesized audio per chunk, through the deployment's chosen speech service.
- `<audio>` plus Media Session, so playback continues with the screen locked.
- Next and previous track mean next and previous section.
- An optional image proxy, if documents have started carrying figures.

**Phase B, only if needed: the durable index.** About a week more. It covers deployments without a
message cache and keeps versions across restarts, with the erasure rules above.

**Phase 4: with logbook (design C).** Gated on that milestone.

- Documents as a fact source.
- Abstracts on cards through the summary ladder.
- A `documents` capability for research.
- Optionally a "Discuss" button that gives the voice agent the document as fenced context, with the
  owner's privacy answer from question 3.

## Related work

Public systems only.

- **Artifacts in single-vendor agent apps.**
  - Claude's Artifacts and ChatGPT's Canvas show model-produced documents in a panel beside the
    conversation, with earlier versions kept as the model revises them.
  - The document lives inside the vendor's own conversation store. That makes versions and titles
    easy for them: there is one writer and one store.
  - Our documents live on ordinary websites, written by any agent, linked from any chat service.
    So we reconstruct the list by recognizing links, and reconstruct versions from the host's own
    history (git), and we cannot assume one writer.
- **Read-later apps with listening** (Instapaper, Readwise Reader, and the like). They save a link,
  extract the text, show a clean reading view, and offer text-to-speech that resumes where you
  stopped and plays as media. The documents view is that app with the "save" step done by
  recognizing links in chat. Their media-style playback is the model for phase 3.
- **Feed readers** (RSS/Atom). The same pattern: a list of cards of documents published elsewhere,
  newest first, unread state kept locally, full text fetched and shown in a reader. Here the
  "feed" is extracted from chat.
- **Link unfurling and URL previews in chat services and servers** (Slack's unfurls, Discord's
  embeds, Matrix homeservers' URL previews). A server fetches a linked page and shows a summary
  card. Matrix's server-side preview is the public model for the fetch safety issues above: a
  server fetching URLs that others post must refuse internal addresses and cap sizes. Those
  systems preview any URL, shallowly. We fetch only allowlisted document hosts, deeply, with the
  owner's credentials, which is why access control matters more here.
- **Forges' own Markdown rendering** (GitHub's renderer, GitLab's). They are the reference for what
  a repository document should look like: heading anchors with a `user-content-` style prefix,
  relative links and images resolved inside the repository at the viewed ref, and sanitized output.
  The document render profile follows them.

## Questions for the owner

1. **Scope of the list.** One list across every channel on the server (recommended, with a channel
   filter), or per channel like the other views? Documents linked by anyone, or only by agents and
   you?
2. **Which version.** Show the newest version with "updated after it was linked" (recommended),
   or the version current when it was linked?
3. **What Read means, and which voice may hear a private document.** Plain read-aloud by section
   (recommended for now)? Or the conversational agent reading it and taking questions about it
   (phase 4)? And for private documents: device voice only, your self-hosted speech stack, or the
   commercial voice API too?
4. **Who may see documents.** Owner surfaces only, with no read-token access and no voice-agent
   tool for now (recommended)?
5. **The car copy.** Keep the newest N documents on the phone for offline reading? That is private
   text at rest in the browser profile, as saved messages already are. If yes, how many?
6. **The paste service.** May the internal deployment's server run the paste service's CLI as
   you, on the server host, to fetch linked pastes?
7. **The switcher.** Is the three-item picker on today's toggle the right interim? Or should
   documents wait for the bottom-bar redesign (`channel-bars-layout`)? Is Ctrl+5 acceptable for
   Documents?
8. **Coverage.** A+ needs the message cache, so the Google Chat deployment would get no documents
   until it runs one, or until B. Is turning the cache on there acceptable? Which repositories
   should the least-privilege GitHub token read?
9. **Other sources.** Is Google Docs worth Google OAuth on the server (one link in 12 days)? Should
   GitHub issue and pull-request descriptions count as documents? Should links to code files inside
   a document (109 to `.json` in the sample) open on GitHub (recommended), or in the reader as
   highlighted code?
10. **Order.** Should a document jump back to the top each time it is linked again (recommended:
    newest mention), or stay where it was first linked?
