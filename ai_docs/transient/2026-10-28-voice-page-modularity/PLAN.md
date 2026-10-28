# voice-page-modularity-plan: final refactor plan

[gpt-6-astra] Final judgement for **#623 voice-page-modularity-plan**, 2026-10-28.
Implementation belongs to **#624 voice-page-modularity**. This document specifies
future work; it does not claim the modules, build pipeline, or gates exist yet.

## One-page summary

**Decision: use Opus 5.5's plan as the migration foundation, strengthened by
Muse's instance ownership and dependency injection.** Opus identifies the hard
parts more precisely: cross-written globals, column switching, source-patching
tests, and browser walks that serve source files. Muse more clearly separates
portable behavior from browser capabilities and makes independent instances
possible. Neither proposal is sufficient unchanged. In particular, neither may
defer the owner's hashed filenames, continue feature lanes through the refactor,
or export a mutable store and call that state hiding.

The outcome is **TypeScript ES modules with small public interfaces**, built by
pinned esbuild into one minified classic script and one stylesheet, both with
content-hashed filenames and linked source maps. The Rust binary embeds the
verified build output. The HTML shell remains revalidated, naming those assets;
installed-app identity and existing plain asset URLs keep working. Retain the
current runtime compression during this refactor. Precompression is useful but
independent work.

The architecture has five boundaries: portable foundations and contract; portable
domain services; browser adapters; browser views; composition. The API and sync
clients, canonical channel state, search language, read-aloud decisions, voice
session logic, read marks, bookmarks, outgoing sends, and settings are reusable
unchanged by a later React Native client. Core code cannot import DOM libraries,
browser adapters, or views. Ports describe capabilities rather than browser
classes. Core services are constructed per application instance; private state
is exposed through read-only snapshots, typed subscriptions, and named commands.
No inheritance or application framework is needed.

Keep roughly a dozen domain packages and a handful of view packages, with small
internal files. A 22,000-line channel package may be a temporary ownership
boundary, but never a single file. Columns initially retain their synchronous
switching semantics behind a private context; async entry points subsequently
capture their context explicitly. The core never knows which DOM column is
active. The DOM is no longer where the selected channel or message model lives.

**P0 sequencing is binding.** Finish the explicitly admitted quick page fixes
before the first implementation move, record the resulting baseline, and stop
all other page-feature work until the refactor's exit gate passes. The older
issue text saying “no hurry,” its numeric priority, and both proposals' concurrent
feature-lane assumptions are superseded by the owner's P0/freeze direction.
Server-only work can continue if it does not disturb the frozen client contract.

Start with test-loader compatibility, a build-only prototype, then the served
bundle. Extract pure modules and ownership boundaries next; migrate stateful
services before the views. Each numbered step below is a small series of green,
independently landable commits. Retain behavior, wire formats, storage keys,
request timing, stale-result checks, DOM IDs, keyboard actions, and CSS ordering.
Do not combine the refactor with the new sync protocol or new views.

Budget **37–59 focused engineering days**, including review and validation,
roughly 8–12 weeks for one implementation lane, with uncertainty concentrated in
the view/column work. Expect about 60–90 small commits; measure gate time before
promising elapsed dates. Completion means zero legacy bridge, strict handwritten
TypeScript, enforced import/state boundaries, a headless portable core, and the
existing behavior and negative-control suites passing against the new pipeline.

## 1. Comparison and evidence

Both proposals were read in full, including their summaries, interfaces,
migration sequences, risks, and the proposers' common brief. The later owner
directions on state hiding, TypeScript, native portability, hashed filenames,
P0 sequencing, and the feature freeze take precedence over their alternatives.
The independent proposal drafts remain external inputs; their private scratch
locations and machine-specific observations are not part of this public record.

### Evidence baseline

The proposals describe a 61,039-line page. This judgement checked the source at
`801f33651bacac3948c274b7cf267797d295ee28`, where it is already **61,500 lines /
3,064,167 bytes**. `contract.js` is 14,835 lines / 459,765 bytes; `voice.css` is
10,366 / 421,718; `voice.html` is 4,070 / 298,491. Line references below use that
commit, unless explicitly attributed to a proposal. Re-resolve symbols at the
implementation freeze; do not run a line-number move script against these old
coordinates.

| Actual subsystem | Evidence in `vibe-talk/web/voice.js` | Architectural consequence |
| --- | --- | --- |
| Bootstrap, identity, screens and navigation | `takePairingCode` at 41; `el` at 236; navigation/screens around 796–2800 | Pairing-fragment removal must remain before any requests; browser history stays outside core. |
| Shared state and rendering | State block around 2801–2960; scroll/folds around 3129–5770; channel/thread rendering from 26161 onward | Separate canonical data, user annotations, and view state; splitting by contiguous section alone cannot do it. |
| Search | Language at 5771–6312; UI applying it follows; server search around 42761 | Pure language first, then a coordinator for searches; DOM hit highlighting stays in view. |
| Composer, keyboard and API | Composer around 10937; keyboard at 11629; `api` at 14898; `within` at 15024 | API owns error taxonomy; UI wording takes explicit context rather than reading the channel selector. |
| Call, confirmations and outgoing sends | Session near 575; confirmation flow from 15122; call around 20613–23990; outgoing placement around 31098 | Several owners are interleaved. Separate call transport, send lifecycle, and confirmation rendering. |
| Canonical channel state | `channelCanon` / `emptyCanon` at 24338 / 24340; `mergeIntoCanon` at 24416; `foldTimelineDelta` at 24857 | Keep merge/deletion/coverage rules and unchanged-message object identity. A delta omission is not deletion. |
| Read state and bookmarks | Bookmark at 34502; read modes at 44487; read-state propagation at 46143 | One annotation owner shared by all projections, separate from DOM row state. |
| Read-aloud | `fetchSpeech` at 38336; `readingBlocks` at 39009; `readPieces` at 39275; `readAloud` at 40218 | Separate prepared block/piece decisions from DOM block discovery and audio execution. |
| Columns and thread cards | Columns at 49268; `COLUMN_STATE` at 49533; `columnSteps` at 49635; cards through 54075 | Async work currently depends on explicit resume/pause around awaits. Preserve it before changing representation. |
| Sync and settings | Freshness at 54076; stream at 54882; all-channel stream at 56328; synced keys at 59126 | Sync is an orchestrator, not another store; settings values and credentials are not DOM state. |
| Wiring and mobile keyboard | Wiring at 59606; keyboard at 60265 and 60563 | Preserve registration and startup ordering while giving every mount a disposer. |

A simple current-source inventory finds 168 section headers, 403 unindented
`let` declarations, and **43 entries** in `COLUMN_STATE`. These are inventory
counts, not a complete semantic dependency analysis. Opus's 45-column-slot and
transitive-coupling figures describe its analysis of an earlier snapshot; do
not present them as independently reproduced facts about this one.

### What to keep, and what to correct

| Topic | Opus 5.5 | Muse | Final judgement |
| --- | --- | --- | --- |
| Migration mechanics | Detailed ordered fragments, accessors, bridge, blame discipline, and negative-control rebuilds (§§5–6). | Faster path to real modules, harness first (§§5–6). | Base on Opus, but use temporary fragments only where needed. Do not spend an entire phase creating 80 global-script files before establishing ownership. |
| State hiding | Useful owner map, but example `Store.set`, exported mutable `ChannelStore.messages`, and global `events` expose writes (§2.4–2.5). | Constructed instances and injected collaborators are stronger (§2.3); “immutable-ish” snapshots remain underspecified. | Factories with private state, deeply read-only published records, stable identity, named commands, scoped listeners, and no exported singleton bus. |
| View granularity | Channel internals split into smaller files; acknowledges their coupling. Permits internal cycles. | Proposes a 14k-line timeline file and 8k-line screens file. | Keep coherent packages with small internal files and forbid cycles at file level too; use callbacks or move a shared operation to its owner. |
| Portability | Separate platform package and a DOM-less TypeScript check. | Explicit injection and two-instance testability. | Combine both. Do not add ambient browser-shaped declarations just to make core compile. |
| Tests | Correctly identifies source-text mutation and walks serving `--web-root`. | Says browser walks must never change and source tests never need built output. | Preserve behavioral assertions, but adapt loaders, source mutations, and asset URLs. Production minification must receive real browser coverage. |
| Build and delivery | Concrete Cargo/embedded-output integration; measured minification/compression experiments. | Sensibly keeps runtime compression, but proposes committing dist and delays served-bundle cutover until after module extraction. | Generated dist is untracked; Cargo builds or verifies it. Serve the bundle before the first imported module. Retain runtime compression to isolate risk. |
| Cache names | Keeps `?v=` and makes filename hashing an owner question. | Initially promises hashed filenames, then defers them. | Implement hashed filenames at the first bundle cutover. This is settled direction. |
| Concurrent work | Extensive hot/cold lane scheduling and retarget tooling. | Also assumes continuing feature lanes. | Freeze page features. No need to build a general-purpose rebase tool for a concurrency policy the owner has rejected. |

Two source checks materially change the proposed interfaces:

* **`readingBlocks(box)` is not portable.** It walks `node.children`, reads
  `tagName`, and returns `{element, kind}`. Both proposals put it among portable
  speech helpers. Keep DOM discovery/highlighting in `view/channel/reading.ts`.
  Core consumes prepared block descriptors and plain text, emitting a message,
  block and piece position. Reuse the Rust fixture to check the mapping across
  that boundary, rather than transplanting element-shaped objects into core.
* **The test seam is larger than the classic-script loader.**
  `tests/js/voice_page.test.mjs:118–156` reads HTML, CSS and Rust sanitizer data;
  `:4671–4697` evaluates globals and patches source. The separate
  `send_double_check.test.mjs:11–22` also slices helpers from the source.
  `tests/column_overscroll_browser.py:280–301` patches source and reads a constant;
  `fold_steps_browser.py:143–153` serves files from its web root. Counting only
  page-suite tests or changing one loader would miss these consumers.

The server already compresses text, including `application/json`
(`src/http/mod.rs:109–165`). Muse's proposed extra JSON MIME allowance is
unnecessary. Opus's approximately 0.29 MB estimate combines minification,
HTML comment removal and brotli-11; it is not a measured result of this final
plan. Record actual raw, gzip and brotli-5 sizes in the build prototype.

## 2. Target packages, public interfaces, and ownership

All paths below are relative to `vibe-talk/web/src/`. A **package** is a directory
with one `index.ts` public entry, not an npm package. Handwritten application
code is `.ts`. Use descriptive internal files, generally 300–1,500 lines;
review a file over 2,000 lines for a real responsibility boundary. These are
review thresholds, not reasons to fragment a coherent algorithm mechanically.
Size ranges are allocation estimates with comments, not measured extraction
sizes or a commitment to retain every line of duplication.

| Package | Approximate size | Owns | Public surface, in outline |
| --- | --- | --- | --- |
| `foundation/` | 1–2k | Stateless text, IDs, time transformations, private observable implementation | Pure functions; `Readable<T>` type. No public application-wide store or event bus. |
| `contract/` | Generated | Wire types and standalone validators | `decode`, `is`, `decodeTagged`, exported types; no mutable validator internals. |
| `ports/` | 200–500 | Capability interfaces only | HTTP, byte/text stream, clock, cancellation, storage, visibility, audio and socket contracts. |
| `core/api/` | 0.8–1.5k | Request accounting, decoding, timeout/error semantics | `createApi(ports, credentials, callbacks)`; typed `request`, `decoded`, `within`; disposal. |
| `core/settings/` | 1–2k | Device/synced settings, key migrations, channel registry | `createSettings`, `get`, named `set`, `applyServer`, `syncedSnapshot`, `subscribe`. Credentials have a separate private handle, never a general settings snapshot. |
| `core/navigation/` | 0.5–1k | Selected channel and logical place, deep-link interpretation | `selectChannel`, `openPlace`, `snapshot`, `subscribe`; typed `Place`. No browser-history stack or pixel positions. |
| `core/channels/` | 3–5k | Canonical messages/threads, tombstones, placement knowledge, coverage, current cache semantics | `applyPage`, `applyDelta`, `applyLive`, `project(query)`, `snapshot(channel)`, `subscribe`; all mutation inside the owner. |
| `core/annotations/` | 2–3k | Read/Done/put-back, bookmark, pins/saved/archive state and in-flight acknowledgement bookkeeping | Named commands (`markRead`, `setDone`, `moveBookmark`, etc.), `applyServer`, `snapshot`, typed change subscription. Existing server authority is preserved. |
| `core/search/` | 1.5–2.5k | Query parser/matcher/planner; search jobs and cursors | Pure `parse`, `matches`, `upstreamPlan`; constructed search controller for network/coverage work. |
| `core/outbox/` | 2.5–4k | Pending sends, destinations, retry/outcome/undo state, draft data, confirmations | `enqueue`, `retry`, `dismiss`, `undo`, `confirm`; immutable tickets and send snapshots; typed acknowledgement callbacks. |
| `core/speech/` | 2–3k | Pieces, segments, read queue, position, recovery, prefetch decisions | `readPieces`, `speechSegments`, `createReader`, `read`, `next`, `previous`, `stop`, `snapshot`. No HTML parser or DOM node. |
| `core/call/` | 3–5k | Logical voice session, reconnection, transcript/recap/confirmation decisions | `createCall`, `start`, `hangUp`, `resume`, `sendText`, `snapshot`, typed events; audio/socket handles stay private behind ports. |
| `core/sync/` | 1.5–3k | Poll/stream scheduling, freshness, generation and cancellation bookkeeping | `start`, `watch`, `refresh(reason)`, `stop`, `snapshot`; calls owners' command capabilities, never edits their state. |
| `platform/web/` | 1–2k | Browser capability implementation | Factories for ports, credential storage, media resources and location/build metadata. Only `app` constructs them. |
| `view/dom/` | 1–2k | Typed shell lookup, notices, common small DOM operations | Typed element lookup and scoped mount helpers. No domain state, service locator, or feature routing. |
| `view/channel/` | 15–22k across internal files | Rows, cards, replies, columns, scroll anchoring, DOM block discovery and selection presentation | `mountChannel`, `open`, `focusMessage`, `selection`, `dispose`; internal `rows/`, `cards/`, `replies/`, `columns/`, `scroll/`, `reading.ts`, `search-hits.ts`. |
| `view/composer/` | 2–3k | Editor elements, growth/focus, draft presentation and send controls | `mountComposer`, `focus`, `present`; invokes a narrow outbox/draft capability. |
| `view/voice/` | 2–4k | Call and read-aloud presentation | `mountVoice`, focus/visibility operations; consumes call and speech snapshots. |
| `view/settings/` | 2–4k | Settings/help/sign-in presentation | `mountSettings`, `open`, `close`; owns its listeners and UI state. |
| `view/shell/` | 2–4k | Top-level screen mounting, keyboard dispatch, browser Back/Forward and viewport handling | Explicit view registration, typed navigation/focus commands, `dispose`. No channel renderer dependency in shared navigation types. |
| `app/` | 0.5–1k | Construction, subscription wiring, boot order and teardown | `createApplication(ports)` then explicit `mount`/`start`/`dispose`. No business rules. |

### State must actually be hidden

Use factory closures or private class fields as convenient. No exported mutable
singletons; no exported generic `set`, `update`, map, array, or `getState()` that
returns the live mutable object. Consumers receive read-only query results and
commands. For example, this is a proposed boundary, not an exhaustive API:

```ts
export interface Readable<T> {
  snapshot(): T;
  subscribe(listener: (next: T) => void): () => void;
}

export interface ChannelSnapshot {
  readonly channel: string;
  readonly messages: readonly ReadonlyMessage[];
  readonly coverage: readonly CoverageRecord[];
}

export interface ChannelCommands {
  applyPage(input: PageInput): ApplyResult;
  applyDelta(input: DeltaInput): ApplyResult;
  applyLive(input: LiveInput): ApplyResult;
}

export interface ChannelReader extends Readable<ChannelSnapshot> {
  project(query: ProjectionQuery): readonly ReadonlyMessage[];
}

export function createChannelState(deps: ChannelDependencies): {
  reader: ChannelReader;
  commands: ChannelCommands;
  dispose(): void;
};
```

`ReadonlyMessage` includes nested read-only fields; shallow `Readonly<T>` is not
sufficient for nested arrays/maps. Do not expose a `Map` disguised as a frozen
object: freezing a map does not disable `set`. Publish records/read-only
collections without writable aliases, using structural sharing. Freeze published
records in development tests, including newly decoded inputs adopted by owners.
Keep the existing identity of unchanged messages (`mergeIntoCanon:24420`) so a
no-op refresh does not redraw every row. Views receive reader capabilities;
sync/outbox workflows receive only the commands they require.

Subscribers run synchronously in the existing order during extraction. Register
them before starting network activity. Preserve the current observable order of
mutation, redraw, persistence and notices, including errors. Do not silently
introduce microtask batching, debouncing, transactions, or a different exception
policy. Characterize a re-entrant path before replacing its direct calls.
Subscriptions and resources have explicit disposal; sign-out disposes the old
auth-scoped application, cancels its work and drops its private cache before a
new instance can publish data. Late completions must obey the existing epoch /
generation checks even when cancellation fails.

The annotation store owns annotation facts. The channel store owns server
message facts. A projection can combine their read-only snapshots; it does not
create a second independently writable copy of read state. The outbox owns send
lifecycle and a typed destination captured at enqueue time, never a later read
of the selected DOM channel. `core/navigation` owns selection; each column owns
its own view query, cursor and presentation state. Views may keep DOM references
in private weak maps. That is not a license for core to get messages back from
those nodes.

### Portability contracts

* Core compiles in a separate TypeScript project with `lib: ["ES2022"]`,
  `types: []`, and no browser or Node ambient declarations. `ports` and
  `foundation` are included in that same check. Do not recreate `window`,
  `document`, `AudioContext`, `Response`, `ReadableStream`, or DOM `AbortSignal`
  as ambient escape hatches. Use small structural request/stream/cancellation
  interfaces and opaque resource handles; adapters convert them.
* Inject clock/timers, randomness used for operation IDs, storage, HTTP, live
  text streaming, socket I/O, audio capture/playback and app visibility.
  Distinguish live text streams, TTS byte streams and call sockets. Native
  support is an adapter responsibility, not an assumption about native `fetch`.
  Audio ports specify sample format/rate, end/error/cancel, scheduling clock
  and stop completion, so core can keep its current gap/recovery decisions.
* Preserve synchronous preference-read behavior via a hydrated in-memory
  snapshot supplied before core starts. The web adapter initially preserves
  today's localStorage write/error behavior. Native write-behind must report
  persistence failures and provide flush completion; do not promise a durable
  write merely because a memory map accepted it. Durable-send/offline semantics
  remain a separate design.
* Prepared speech data and logical reading position are portable. Measuring or
  traversing rendered HTML, mapping a piece to an element, scrolling it into
  view, audio devices and platform speech engines are adapters/views. Pure
  glyph-width heuristics may move unchanged; actual layout measurement may not.
* Core imports must be inert: construction and `start()` are explicit. The
  composition root removes a pairing fragment before any request and hydrates
  state before subscriptions/startup reads. This ordering is a tested contract.

### Columns: preserve first, then confine

Start by putting the current `COLUMN_STATE` fields behind a private per-column
record in `view/channel/columns`. Switching one pointer replaces the existing
copy-in/copy-out representation; `inColumn` still restores the prior context in
`finally`, and `columnSteps` still resumes/pauses around every wait. This changes
representation, not the sequencing contract. Preserve fresh-field values and
each array/set's allocation identity.

Then migrate one async call chain at a time to a captured `ColumnController`:
its query, generation, element scope, and commands belong to that instance.
Two-column interleaving tests must pass at each chain conversion. Remove ambient
column dependence from exported interfaces; any remaining synchronous pointer
helper is private to the channel package. Do not pass `ColumnState` objects to
arbitrary packages for mutation. Explicit context is the endpoint; a full rewrite
of all column algorithms at the initial extraction would be too risky.

## 3. Dependency rules and enforcement

An arrow below means “may import.” All cross-package imports use `index.ts`;
type-only imports are subject to layer rules as well.

```text
app -> core public interfaces, platform/web, view public interfaces
view/shell -> leaf views through injected commands; view/dom; core read/command types
view/channel, view/voice, view/settings -> view/composer, view/dom; core public APIs
view/composer -> view/dom; core public APIs
platform/web -> ports, foundation, contract types
core services -> ports, foundation, contract; allowed lower core public interfaces
ports, foundation, contract -> no platform, view, app or core services
```

Within core, API and settings have no dependency on domain services. Navigation,
channels and annotations are peers; search reads channel/annotation projections;
outbox receives channel/annotation command capabilities; speech receives API and
settings capabilities; call receives outbox/speech capabilities; sync receives
the owners' command capabilities and call-state observations. Prefer injected
interfaces for collaboration rather than importing a constructed instance.
Move shared projection types down to their owning public interface, never up
into the composition root. Neither channels nor annotations imports sync.
Views do not import `platform/web`. Sibling feature views communicate through
typed commands wired by `app`, not circular imports or a universal event bus.

Pin ESLint and its TypeScript support alongside the compiler. Enforce restricted
imports/globals and explicit type imports. Add a small TypeScript-resolution
based architecture check over resolved paths and the esbuild metafile: reject
deep imports across package boundaries, prohibited edges, file-level cycles,
unresolved/dynamic module edges, and view/platform dependencies in portable
entry points. Bundling alone does **not** prove an acyclic graph. Tests for this
checker include deliberately bad imports and cycles, including aliases and
re-exports, so it fails closed.

Type-check ownership as well as imports: read-only public records, no exported
mutable bindings, no `any` or blanket suppressions in handwritten core APIs,
and negative type examples rejecting attempted foreign writes. Private generic
store machinery is permitted inside an owner; it is not part of its public API.
Run these cheap checks from the same validation DAG as typechecking. The legacy
boundary has an explicit, shrinking exception inventory; new code cannot add an
exception implicitly.

## 4. Build, Rust embedding, assets, and cache behavior

### One build definition

Use **esbuild**, pinned in the existing `contract/package.json` and lockfile,
beside TypeScript 5.9.3 and Ajv. Reuse the current install route initially rather
than add two competing TypeScript installations. The build resolves tools from
that lockfile, never a system executable. Update that package's description to
cover frontend tooling. A later neutral directory rename can be separate.

`web/build.mjs --out DIR` is the single build definition used by Cargo, tests,
browser fixtures and packaging. It bundles `src/app/main.ts` to one classic IIFE
and combines CSS in its existing order. During migration, the entry also loads
one legacy source/manifest; only the test variant adds explicit test accessors.
No code splitting, framework runtime, CDN dependencies, service worker, new CSP,
or HTML minifier is needed for the refactor. Preserve current script execution
timing. `tsc --noEmit` performs typechecking independently of esbuild.

Output is ignored, under `target/web/` for local tests or `OUT_DIR/web/` for
Cargo. Each build writes a private temporary output directory and publishes it
only on success. Concurrent Cargo/test builds do not write the same directory.
The output contains:

* `/assets/voice.<content-hash>.js` and `.css`, minified;
* separately content-hashed linked `.js.map` / `.css.map` files;
* processed `voice.html` referencing the manifest's exact asset filenames;
* `assets.json`, with logical names, paths, MIME types, digests, build inputs,
  tool/lockfile identity and the page-build identifier;
* compatibility artifacts for the four old plain JS/CSS routes, and the
  existing manifest/icons needed by a fixture web root.

Avoid hash self-reference: produce external maps with relative source paths
and no final hashed `file` name, hash/name the map, append its `sourceMappingURL`
to the asset, then hash/name the final JS/CSS bytes. Use SHA-256 truncated to
128 bits as today, detect collisions in the manifest, and verify digests after
writing. `page_build` is the final JS digest; do not compile that digest into
the bytes being hashed. The web adapter reads the current script's hashed
filename (and accepts legacy `?v=` for compatibility). Source maps contain
repository-relative paths and source content, never absolute build paths.
Test a real thrown location resolving to the correct source line.

### Cargo and server integration

Extend the existing `build.rs`, preserving its syntax-grammar build. By default
it runs `node web/build.mjs` into `OUT_DIR/web`. Missing prerequisites fail with
an actionable command, without downloading packages from a Cargo build script.
Emit `rerun-if-changed` for source/input directories, shell, styles, generator
outputs, build script, manifest and lockfile; additions/deletions must invalidate
the build too. Emit `rerun-if-env-changed` for the prebuilt-input variable.

A `VIBE_TALK_WEB_DIST` alternative supports a packaging Node stage. It is not
an unchecked bypass: compare its input/toolchain fingerprint to the current
source and lockfile, verify every declared output digest and required entry,
then copy into `OUT_DIR/web`. Reject stale/missing output. Generate the Rust
asset table there with literal `include_bytes!` paths; Rust macros cannot
discover arbitrary filenames from JSON at runtime. `assets.rs` includes that
generated table. Do not require deployed servers to have Node or a writable
web directory. Do not commit minified dist to Git.

Add a pinned Node build stage to the Containerfile and supply verified output
to its Rust stage. Include frontend source/tool inputs in packaging inventories.
Test the clean-checkout and prebuilt paths, including a deliberately stale
prebuilt manifest. Cargo-only distribution now requires Node plus installed
locked tooling, or verified prebuilt assets; update build/help documentation
and release packaging together with the cutover.

In `src/http/assets.rs` and `mod.rs`, add one bounded `/assets/{name}` route
whose names are looked up in the generated table. It is never a filesystem
server. Keep scope-first route tests, content types, weak ETags, `Vary:
Accept-Encoding` on both 200 and 304, and existing compression. The current
`link()` query rewriting remains for unaffected icons/compatibility routes;
the new shell's bundle paths are already resolved and must not acquire a second
version query. Naming substitution for title/sign-in/manifest still runs with
its existing escaping and “exactly once” tests. Shell and install manifest
remain `no-cache`; PWA `id`, start URL and scope remain unchanged.

### Compatibility and mixed-cache cases

Only the exact current hashed asset names are immutable for a year. Keep plain
`/voice.js`, `/contract.js`, `/voice.css` and `/style.css` revalidated. The
legacy `/voice.js` route can serve the new complete bundle. **Do not make
`/contract.js` empty:** a browser may have the old voice script cached while
fetching a missing contract script. Retain a standalone generated legacy
contract wrapper; the new bundle's private contract must coexist with it.
For CSS, keep the base-only and voice-only compatibility artifacts in the
original order; do not accidentally apply a second base sheet after component
rules. New shells load one combined sheet.

For a stale but syntactically valid `voice.<hash>.js` or `.css` name, use the
existing stale-query policy: serve the current corresponding bytes with
`no-cache` and the current ETag. Never label different bytes immutable under
the old hash. The requested old script hash still lets the existing stale-page
notice invite a reload. Unknown asset families and malformed names are 404;
stale map names are also 404 rather than symbolicate with the wrong map. There
are no lazy chunks that could mix versions during an already-running session.
If permanent old-version debugging is desired, retain release artifacts outside
the served binary; it is not required to boot an old shell.

Exercise old shell + cached old voice + fetched contract, old shell + new voice,
current shell, stale hashed URLs, unrelated URLs, and rollback fixtures. Keep
API compatibility policy unchanged. This refactor cannot make an arbitrary
future incompatible server work with every historical cached client.

Stage the initial server cutover: first land the generated asset table,
compatibility routes and dual-format `PAGE_BUILD` reader while still serving
the old shell; switch the shell in a second green commit. The preparatory build
must be the supported rollback floor for the new hashed shell. A binary from
before those routes existed cannot serve a new cached shell's asset paths;
do not claim arbitrary pre-refactor rollback compatibility. Qualify and record
the rollback pair before any implementation deployment.

## 5. TypeScript, CSS, and HTML

New core modules are strict from their first landing. Use `isolatedModules`,
`verbatimModuleSyntax`, erasable TypeScript syntax, and no emit for checking.
The legacy JS stays under the existing `checkJs` configuration while it shrinks.
Convert JSDoc to TypeScript as each owned module is extracted; a rename alone
loses JSDoc type checking in `.ts` and is not a green intermediate step. No
blanket `@ts-nocheck` migration. Finish with strict handwritten view code too;
type-only narrowing work may land separately from its mechanical move.

The Rust schema remains the contract authority. Update `contract/generate.mjs`
to emit module types and an ESM validator surface. A small generated `.js`
validator implementation with generated declarations behind `contract/index.ts`
is acceptable machine output; all handwritten modules are `.ts`. This avoids
pretending Ajv's unannotated generated functions are strict handwritten TS.
Keep the old standalone wrapper for compatibility delivery only. Contract
freshness, sample acceptance/refusal, unknown-tag handling and lack of runtime
Ajv dependencies remain tested. The portable check must traverse the wrapper's
dependency graph, including generated JS.

Split CSS mechanically after the build and view boundaries are stable. Preserve
`style.css` first and every `voice.css` rule's original position using an ordered
manifest of complete top-level rule groups. Do not cut inside media/keyframe
blocks, move late overrides next to early rules, adopt CSS modules, rename
selectors, or “clean up” the cascade. Check byte equality of the reconstructed
unminified CSS before and after a move. Minification was already qualified at
the bundle step. Browser layout walks cover the resulting production CSS.

Keep one source `voice.html`. It is a static shell and a useful independent
inventory for the fake DOM. Only replace asset references in the build; defer
comment stripping and whitespace/template rewrites. Generate typed ID lookup
from parsed live markup, excluding comments and checking duplicate IDs and
element kinds. Keep an independent HTML-to-generated-type parity test and the
fixture's “unknown ID throws” rule. Generated code can be wrong; generation is
not a reason to delete the oracle. Remove DOM expando declarations as private
view weak maps take over. Do not change IDs, labels, nesting, or sanitization
behavior during extraction.

## 6. Test strategy and green gates

The existing behavior suites remain the primary regression net. New tests are
for new architectural and loading guarantees, not copies of moved function
bodies. A pure move retains assertions and negative controls.

1. **Inventory source readers before moving code.** Search `src`, `tests`,
   scripts, fixtures and packaging for the four asset names, `VOICE_JS`, script
   evaluation and marker cuts. Track each consumer's new loader or module.
   Known Rust consumers include `search_query.rs:917,994–1050`,
   `client_settings.rs:227`, `http/api.rs:3467–3675`, and
   `tests/elevenlabs_mock.rs:807`. Also include the three JS test files,
   browser mutation controls, route/asset/signed-URL/rendering tests, PWA checks,
   and the Rust page-suite launcher. Re-run the search at bridge deletion.
2. **Shared source/build helper.** Initially assemble exactly today's sources
   in their original order. Once modules appear, produce an unminified IIFE
   through the same build graph for the fake-DOM VM. Keep production and test
   entry differences explicit. No concatenation of ESM text as though imports
   were classic scripts. The test build exposes only an explicit accessor
   inventory for legacy `page.pure` uses; append these inside the legacy scope
   before bundling, without `eval`. Production has no test globals or private
   state exports. Migrate pure assertions to direct module tests over time.
3. **Preserve negative controls.** Replace `brokenScript` with a source-overlay
   build that patches the original module before transformation. Assert the
   expected match count (including intentional multi-match controls), reset
   overlays between cases, rebuild, then require the intended behavioral
   assertion to fail. A missing needle or syntax error is not a successful
   negative control. Python browser controls need the same source-overlay-to-
   temporary-dist path, not a replacement in minified bytes. Benchmark rebuild
   time/memory with the whole control set before production cutover.
4. **Portable module tests.** Build ESM test output with the pinned toolchain
   and run `node --test`; do not depend on whichever Node type-stripping feature
   happens to be installed. Reuse search and speech fixtures and the seeded
   Rust search differential. Test two instances, disposal/late replies,
   unchanged-object identity, foreign-write rejection, and no global DOM or
   timer capability. Plain-data stores/commands need no fake DOM.
5. **Rust marker tests.** During transition use one explicit source manifest
   helper instead of scattered new paths. Switch executable language checks
   to importing the built pure module with the same corpus. Compare generated
   settings-key data and fold tables structurally. Source-shape assertions can
   temporarily read a named owning module; replace them only with equivalent
   behavior coverage, including their controls. Do not point them at minified
   JS or delete them because a marker moved. `assets::VOICE_JS` currently serves
   both embedded output and source assertions; separate those roles explicitly.
6. **Browser walks.** Keep behavioral expectations and IDs. Change file roots
   and asset/mutation loading as required. A `web-build` DAG prerequisite
   supplies a complete fixture root; real-binary walks use embedded output.
   Run the existing selected browser suite on minified production output, with
   particular attention to two columns/overscroll, thread entry/replies,
   keyboard/safe-area, sign-in, send outcome, read-aloud/recovery and call
   lifecycle. Fake audio remains fake; record any physical-device checks that
   the portable gate cannot provide. Verify map symbolication on a browser.
7. **Test organization.** First preserve all current collection and sharding.
   Later extract the fixture and move tests by feature into `tests/js/core/`
   and `tests/js/page/`. Maintain a collected-test and negative-control
   inventory across the move, update explicit file lists/globs and the Rust
   launcher, and retain per-process resource limits. Avoid multiplying a
   heavyweight fixture into dozens of simultaneous processes. No duplicate
   running of the same suite through Cargo and the DAG.

Every push uses the root **`make validate`** contract, run bare or redirected
with its real exit code checked. Update `validation/components.json`,
`scripts/validate.py` selection tests and `vibe-talk/tests.dag.yaml` with each
new input/entry point. The broad `vibe-talk/` path is already classified; verify
the new files select the intended union rather than assume they need a new
top-level rule. Wire build prerequisites into the existing DAG, not a second
list of commands in CI. Keep root hygiene and fail-closed routing. At cutover
and exit, run `make validate-all`; incompatible compiler/container-specific
jobs remain their existing focused checks. Do not infer physical microphone or
phone verification from a headless browser result.

## 7. Numbered migration sequence

Each row is a bounded stage with several small commits where necessary. **Every
commit must remain independently typechecked/buildable and behaviorally green;
every push receives the full selected green gate.** “Move” and “improve the
interface” are separate commits whenever each intermediate state is valid.
The extra gate column names evidence beyond that common requirement.

| Step | Change and behavior-neutral boundary | Additional green evidence | Effort |
| --- | --- | --- | --- |
| 1 | Finish admitted quick page fixes, freeze the page, pin baseline, inventory source readers and state writers. Reconcile implementation issue priority/dependencies with P0. Record behavior and suite counts; change no algorithms. | Baseline full gate; agreed freeze checklist. | 1 day |
| 2 | Add source-manifest/test-loader helpers, initially pointing to unchanged files. Inventory and prototype mutation overlays, including Python walks and the separate send test. | Identical assembled bytes; unchanged test/control collection. | 2–3 days |
| 3 | Pin build tools; add inert esbuild prototype, generated IDs, manifest/digests/maps and architecture-check tests. Server still serves original sources. Prototype a tiny pure module in test-only input to prove ESM + VM accessors + controls. | Both source and bundled harnesses green; measured sizes/rebuild cost; source-map test. | 2–3 days |
| 4 | Add Cargo/prebuilt/container integration, compatible asset routes, dual-format `PAGE_BUILD` lookup and fixture roots first; switch the shell to embedded hashed assets in a second green commit. Retain code order, shell structure and runtime compression. | Full gate, asset/scope/PWA/mixed-cache and rollback-pair cases; clean and stale-prebuilt build checks. | 2–3 days |
| 5 | Move the remaining classic implementation into a single named legacy boundary, with an ordered fragment manifest only where a subsequent extraction needs it. Add a typed composition entry and ordered bootstrap facade. No wholesale 80-file intermediate taxonomy. | Exact reconstructed legacy bytes; initialization-order and pairing tests. | 1–2 days |
| 6 | Extract standalone search language, text/ID helpers and speech piece/segment math one group at a time. Convert each extracted module to strict TS; retarget fixtures and Rust differential in the same green commit. Leave DOM `readingBlocks` in legacy view code. | Existing seeded/fixture parity plus direct core imports. | 2–3 days |
| 7 | Generate the contract ESM facade and module types while retaining the compatibility global wrapper. Replace internal global contract uses by imports/adapters. | Contract freshness, every sample/refusal, unknown tags; old/new script combinations. | 1 day |
| 8 | Introduce narrow platform ports and extract API timeout/error logic. Route raw transport/storage access through adapters in separate groups. Preserve auth-refusal cache clearing and late-answer behavior. | API/error/read-reason tests; cancellation and abandoned-read tests. | 2–3 days |
| 9 | Extract settings/credentials and logical navigation selection. Replace selector reads with a forwarding accessor first, then make owned selection authoritative in one commit. Settings controls become projections. | Hydration, storage-disabled, sign-out, synced-key and channel-switch tests. | 2–3 days |
| 10 | Extract canonical channel owner, beginning with pure merge/projection operations, then cache/coverage/placement commands. Invert direct renderer calls one typed callback at a time. | Current deletion/delta/identity/coverage tests; two-instance and late-channel-response cases. | 3–4 days |
| 11 | Extract read/Done/bookmark/pin/saved/archive ownership from rendering. Preserve request/ack/rollback order and cross-view application. No new revisioned sync model. | Existing conflict/refusal and cross-column read-state tests. | 2–3 days |
| 12 | Extract outbox/drafts/confirmation decisions in small state-machine slices. Capture destination and operation identity explicitly; keep current tick/outcome behavior. | Send/retry/undo/outcome/duplicate and reply-placement tests. | 2–3 days |
| 13 | Extract speech queue/recovery and then voice-session decisions behind audio/socket ports, one lifecycle at a time. Move hardware objects to the web adapter without changing audio policy. | Speech fixtures, interruption/recovery, reconnect/recap/echo/control tests. | 3–5 days |
| 14 | Extract sync scheduling and stream decoding, wiring domain commands explicitly. Preserve poll holds, request reason tags, freshness and generations. | Replay/reset/hidden-page/read-hold tests; stream chunking and disposal. | 2–3 days |
| 15 | Replace column-field swapping with private records by field group. Then migrate async column chains to captured controllers by chain, removing ambient context from public APIs. Neither commit combines this with a layout change. | Two-column interleaving and overscroll walks; late reads on either column. | 4–6 days |
| 16 | Move DOM primitives, composer, channel rows/cards/replies, voice and settings views in that dependency order. Each move takes its own narrow interfaces, mount/dispose and typing. Replace row expandos and model reads through DOM. | Existing feature tests and corresponding production-bundle walks after each slice. | 3–6 days |
| 17 | Finish shell/navigation/keyboard composition and ordered boot. Convert every remaining handwritten file to strict TS, remove legacy test accessors as callers migrate, and delete the bridge/manifest exceptions. | No legacy imports/globals, no cycles/deep imports, complete strict checks. | 1–3 days |
| 18 | Split CSS in preserved order; finish generated ID parity and feature test-file moves. No HTML layout rewrite. This can use disjoint refactor-only work after step 4, but shares the same gate/landing order. | CSS byte-equivalence, unchanged DOM inventory, collected tests/controls unchanged, layout walks. | 1–2 days |
| 19 | Audit completion, update operator/build docs, run exit validation and record measurements/unverified device checks. Remove temporary move tooling that has no remaining purpose. Release the page freeze only after the exit criteria below. | Full portable gate, clean build, package/scope/cache coverage and headless core. | 1–2 days |

The rows total approximately 37–59 working days; a stage that discovers new
behavior first adds characterization coverage and revises its estimate. No
schedule claim relies on a particular machine's gate duration. A read-only
source analyzer can guide extraction but cannot *prove* that arbitrary JS
reordering is free of temporal-dead-zone or side-effect dependencies. Use it
as a diagnostic alongside preserved ordering and executable tests.

The transitional facade is **lexical, explicit and temporary**. The legacy
entry imports public factory/functions, passes its remaining callbacks through
ports, and starts services in the old order. Converted modules never import
legacy files. Avoid a `globalThis` export bridge in production. If ordered
fragments are necessary for a difficult move, preserve their concatenation
exactly, keep a deletion ledger and convert that owner immediately afterward.
The bridge is not a new place to put feature code.

Use `git mv` for whole-file renames, a dedicated move commit for contiguous
relocations, and separate codemods from unrelated formatting. Spot-check
`git blame -C -C` on moved logic. List reviewed mechanical conversion commits in
`.git-blame-ignore-revs`; this aids blame but is not a substitute for retaining
meaningful history. Main stays linear through rebases and ordinary pushes;
never force-push or rewrite a shared move commit.

Track more than line count: legacy bindings remaining, foreign writers removed,
core files passing without DOM/Node types, public mutation escapes, unresolved
source-reader migrations, and tests/controls collected. File count or reduced
`voice.js` length alone cannot establish modularity.

### Exit criteria

* Handwritten implementation is strict TypeScript ES modules; no legacy
  concatenation, production global bridge, ambient cross-package state, or
  import-cycle exceptions remain.
* Core builds and runs with fake ports and two independent application instances
  without DOM/Node ambient capabilities; its package graph is portable. A native
  UI is not required to prove this property.
* All required owners, read-only surfaces, command capabilities and teardown
  paths exist. Columns do not expose ambient active-column state outside their
  package. UI reads of DOM controls do not substitute for domain state.
* Minified assets/maps are generated, verified, embedded and served with tested
  current/stale/plain cache behavior, and stale-build detection still works.
* Behavior, fixture parity, negative controls, browser walks, route inventories
  and selected/full validation remain green. Any physical-device verification
  not performed is named precisely in the implementation handoff.

## 8. Follow-on work fits these boundaries

* **#628 documents-index-server** supplies wire APIs;
  **#629 documents-view-client** adds `core/documents/` for list/fetch/freshness
  and a `view/documents/` package. Its logical reader/block descriptors reuse
  `core/speech`; its web block-to-element mapper is separate. Add a document
  `Place` and register one view in the shell; do not add document conditionals
  to every channel renderer. Preserve the planned user-controlled update of
  displayed document contents.
* **#630 state-sync-model** changes transport reconciliation and persistence
  behind `core/sync`, channels, annotations and outbox. Revision cursors,
  item-demand loading and multi-device merge rules belong there, not in views.
  This refactor preserves today's rules; it does not pre-implement monotone
  revisions, IndexedDB, offline sending or a service worker. Those are
  separately specified behavior changes.
* **#652 sent-reply-joins-thread** lands before the freeze or after completion,
  never during it. Its stable home is outbox acknowledgement -> typed channel
  placement command, keyed by destination and operation identity. Whether a
  result is already applied is owned data. A later fix need not manipulate
  rendered rows or force a view refresh. If the fix is admitted before the
  baseline, preserve and move its tests as part of step 12.
* The owner's **top-level-view follow-up** uses `core/navigation` for logical
  destinations and `view/shell` for presentation/keyboard/history. Channel-local
  Main/Threads/All selection stays separate from top-level Voice/Channel/
  Documents selection. The final issue title/slug was not present in the
  reviewed issue snapshot; record it before creating implementation links.
  The extension boundary does not depend on that administrative detail.

## 9. Risks and remaining owner questions

| Risk | Mitigation / decision |
| --- | --- |
| Moving a declaration changes evaluation order or a synchronous redraw | Preserve order first; test startup, re-entrancy and notifications. Static analysis is supporting evidence, not proof. |
| Column awaits resume into the wrong context | Private context representation first; explicit captured controllers one chain at a time; adversarial two-column completion order. |
| TypeScript interfaces appear read-only but leak writable aliases | Deep read-only public types, no mutable containers/exports, development freezing and negative write tests. |
| A native port compiles but relies on browser timing/storage/audio assumptions | Explicit capability semantics, hydration and failure/flush contracts, fake-port tests, no ambient-type escape hatch. |
| Source-patching tests pass without exercising their defect | Match counts, expected failure tags, baseline-green check, overlay isolation and rebuild-cost measurement before cutover. |
| Stale embedded output or mixed cached scripts break startup | Verified manifest, clean builds, retained legacy contract wrapper and mixed-cache tests. |
| Minification/CSS processing changes behavior | Production-bundle browser coverage; preserve function names initially; no CSS reorder or HTML rewriting beyond asset references. |
| Migration becomes permanent halfway architecture | Shrinking exception ledger, package-by-package completion, P0 feature freeze and explicit bridge-zero exit gate. |
| Original plans' schedule or size claims become promises | Measure the new pipeline and gate, report engineering days separately from wait time, revise estimates on evidence. |

The owner has already decided TypeScript modules, a build step, minification,
source maps, hashed names, portable core, state hiding and P0 sequencing. These
are **not reopened as questions**. The plan makes ordinary implementation
choices above so work can start without another design round.

Remaining choices to record with the implementation coordinator:

1. Which already-started quick page fixes make the freeze baseline? The default
   is the existing admitted blocker list for #624 voice-page-modularity; any
   additional feature waits until after the exit gate. Record the final baseline
   commit and canonical title/slug for the top-level-view follow-up.
2. Are source maps to be served like the public source assets? Default: yes,
   same origin, repository-relative sources, with no credentials or deployment
   configuration embedded. Restricting their delivery is an optional policy
   choice, not a reason to omit map generation/symbolication tests.
3. Is a Node-free *source* build a distribution requirement? Default: local
   source builds use the pinned Node toolchain; release builds can consume
   verified prebuilt output, and the deployed Rust binary has no Node dependency.
   If a distribution cannot accommodate either, resolve that before step 4.
4. Who can perform the final physical-phone microphone, keyboard and remote
   source-map checks? Automation covers behavior and simulated audio; record
   remaining hardware checks honestly without treating them as completed.

No React Native rewrite, new framework, new sync protocol, new document view,
service worker, or unrelated UI change is included in this refactor's delivery.
