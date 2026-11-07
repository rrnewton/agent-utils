# #624 voice-page-modularity: app and boot design for step 17

[muse watermelon] #624 voice-page-modularity. Design only; no step-17 code is
included. Written after the reply-box batch and the folded settings, keyboard
and document-listener batch landed in review.

## Recommendation

Shrink `voice.js` into `app/main.ts` by successive logic-owner moves. Do not
convert the 31.8k-line legacy script to strict TypeScript in place.

The composition root will be:

```ts
export interface Application {
  mount(): void;
  start(): void;
  dispose(): void;
}

export function createApplication(ports: ApplicationPorts): Application;
```

`app/main.ts` will be the only entry that knows the platform ports, the owners
and their order. `voice.js` disappears when its remaining text is only the old
prelude, state construction, mount calls and final start sequence; those last
pieces move into `app/main.ts` in one small, separately gated commit.

## Current facts

At the folded wiring batch, `voice.js` has 31,755 lines, 1,067 top-level
functions and 51 textual `addEventListener` occurrences. The strict TypeScript
tree under `web/src` already contains the core, present, foundation, ports,
platform and view packages. The legacy script remains the composition root:
it constructs state, calls roughly forty mounts, discards every returned
`unmount`, and performs final initialization in textual order.

The first statement must remain `takePairingCode()` until the new entry owns
it. It strips the single-use `#pair=` credential before any request or copied
address can expose it. `page_build` also runs the script without a browser, so
an entry change and its test re-pointing belong in the same final commit.

## Ownership and lane boundaries

- `mu-refactor`: shell, navigation and keyboard composition; application boot;
  column and screen/view state unless explicitly reassigned; legacy imports
  and final deletion of `voice.js`.
- `mu-readcore`: the agreed read and refresh ranges, timeline and Discord
  reads, older/search walks, refresh and live-stream owners. It supplies
  factories to the application through narrow dependencies and does not edit
  `view/channel/index.ts` until it becomes that file's sole writer.
- `mu-docview`: the three-way view selector and Documents view. Its selector
  delegates through the existing view-switch path; `pressViewSwitch` remains
  the legacy dependency until shell composition owns it.
- `mu-tsconv`: strictness and test reorganization for already-moved modules.
  Legacy accessor or prelude deletions it identifies are made by the lane
  removing the last legacy caller.

## Mount, start and dispose

Every current mount already returns an object with `unmount()` based on
`createListening`. `Application.mount()` will call the existing constructors
and mounts in exactly the current textual order and append each returned
teardown to one array. No registry, event bus or sorting is introduced.

`Application.start()` will contain only today's final sequence, in order:
token state, stored bar placement, saved read mode, first view and empty
state, the pairing/sign-in branch, back-stack start, and deep-link opening.
`startBackStack()` remains last among construction steps before the deep-link
call, as it is now.

`Application.dispose()` will pop teardowns in reverse mount order and be
idempotent. Timers, observers, sockets and owners that do not yet expose an
`unmount` get one in the owner move that reaches them. The known insertion
watcher leak in the message and reply boxes is fixed in the owner bundle that
first folds those mounts into `dispose`, as its own behavior-preserving
change: `watchInsertions` must return the teardown for its three listeners.

Two application instances in one test process must not share module-level
mutable state. Factories already receive dependencies; any remaining singleton
found during an owner move either becomes factory state or is named as a
deliberate page-global in the bundle's review.

## Order of owner moves

Each gate carries at least 1,500 lines removed from `voice.js`, except the
final entry/deletion commit, which is small by construction. Parallel
subagents prepare all-or-nothing owner bundles (module, module test and the
`voice.js` excision); one integration applies them in dependency order, one
adversarial review reads the combined diff, and one full selected gate runs.

1. **Shell and navigation composition.** Move `showScreen`, `showView`, the
   back-stack orchestration, visited-place handling and deep-link opening
   behind `view/shell` factories already present (`createScreens` and the
   `core/navigation` owners). Introduce `createApplication` with mount/start
   collecting existing teardowns, but leave `dispose` unused by production
   until every collected owner has one.
2. **Keyboard composition.** Move the desktop key-map dependency graph and the
   on-screen keyboard construction from legacy glue into shell/composer
   owners. The wiring mounts from the folded batch become the mounts called by
   the application. Preserve capture order and the browserless guards.
3. **Column, screen and view state.** Move `currentScreen`, `currentView`,
   column selection and their test accessors behind shell/channel owners.
   Re-point the tests that read those names as page globals in the same bundle.
4. **Remaining channel glue, excluding the readcore ranges.** Draft stores and
   chip, reply state and docking, sent marks, settings orchestration and other
   owners are bundled by dependency, not by screen. Readcore factories are
   consumed through their agreed public dependencies.
5. **Final entry.** When the legacy remainder is construction and start only,
   move it to `app/main.ts`, convert it to strict TypeScript, delete
   `legacy/imports.ts`, `web/legacy-imports.d.ts` and `web/voice.js`, remove
   the legacy package exception from `web/architecture.mjs`, and re-point
   build and test readers in the same commit.

## Test and reader changes in the final commit

- `tests/thread_card_markdown_browser.py:296` rebuilds the pre-build shell's
  `/voice.js?v=<hash>` asset name from `voice.js` bytes. Give it the
  `app/main.ts` built asset name at deletion time; this item is owned here.
- `page_build`'s first-constant and prelude checks must name the new entry or
  be deleted where the built-module tests prove the same property.
- Source-patching page tests move from exact legacy text to module overlays or
  behavior tests in the bundle that moves their target. A negative control is
  never deleted merely because its text moved.
- Python walks that evaluate bare page globals (for example `setStatus`) must
  call the built page/module surface instead before `voice.js` can be deleted.

## Evidence required per gate

- Inner loop: typecheck, module suites, parallel page suite, AST diff of every
  moved declaration, text-reader scan and prelude-drop scan.
- One adversarial review over the combined diff, with explicit checks for
  temporal-dead-zone references, fetch/poll/redraw timing, listener identity,
  return values and disposal order.
- One full selected gate on the integrated tip, with progress reported as
  `voice.js` lines, top-level functions and `addEventListener` occurrences
  remaining.
- At exit: two independent applications with fake ports, strict checks with no
  legacy exceptions, the full portable gate, a clean build, and physical-device
  checks named as unverified if they were not performed.

## Rejected alternative

Converting `voice.js` to TypeScript in place would put more than thirty
thousand lines under strict indexed-access and unused-symbol checks before any
line could move. That creates one huge, behavior-adjacent change and delays
the portable-core proof. Shrinking by owners keeps each commit independently
reviewable and leaves a working classic script until the final entry commit.
