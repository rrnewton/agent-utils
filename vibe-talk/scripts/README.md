# Screenshot harness

`screenshots.py` photographs the app at `/` in every state that looks
different, driving a real Chromium against a real throwaway server.
`run.sh --screenshots` builds the server, starts it with the fake chat
upstream plus two loopback stubs, runs the walk, and stops everything it
started. Nothing touches the live instance, its config, or its database.

## The standard matrix (owner standing acceptance)

Every owner-UI change is evidenced with the same matrix. Do not write a
new screenshot script for it — run the shared states:

```bash
cd vibe-talk
scripts/run.sh --screenshots --theme both \
  --profiles android-412,laptop-1280 \
  --only 73-discuss-preview --only 74-discuss-summary \
  --only 75-discuss-keyboard-closed --only 76-discuss-keyboard-open \
  --out /tmp/<lane>-shots
```

That gives light and dark × phone 412x915 (scale 2.6) and desktop
1280x800, with the Discuss half-pane open in every shot, plus your own
targeted states (add `--only <state>` for them, before and after your
change). Look at every image yourself before handing it over; a run that
writes files nobody opens is not evidence.

What the harness guarantees, so lanes do not have to re-prove it:

- **Discuss is real.** For a Discuss run (`--only` naming the matrix
  states) the throwaway server also gets a serving message cache, and
  the harness seeds it with the server's own history walk; with the fake
  chat upstream serving the timeline and a `[summaries.llm]` section
  pointing at `model_stub.py` on loopback, preview and Summarize answer
  through the real server router with the page's exact requests. Only
  the external model is stubbed. A shot showing "not served by this
  check", a 404, or an error banner for Discuss fails the state's own
  checks. A wider walk keeps its cacheless server (the Settings and
  Help states photograph exactly that) and skips the Discuss states by
  name there.
- **The keyboard really shrinks.** `76-discuss-keyboard-open` uses the
  one shared `shrink_keyboard` helper: viewport set to height − 336,
  then a condition wait until `visualViewport.height` equals it. If an
  open shot's height equals the closed one's, the run fails by name —
  identical open/closed shots are rejected, never filed.
- **No transient UI.** Before every capture the harness settles the page
  (switcher cards and menus put away through the page's own dismissal),
  unless the state targets that UI (`keep_transient`). Stray popups or
  toasts over a shot are a harness failure, not a review finding.

## Checks

```bash
make screenshots-self-test   # offline controls; no browser, no server
scripts/run.sh --screenshots --dry-run
```

The self-test proves the capture judge, the PNG decoder, the scene
table's expectations, and the keyboard-shrink check (including that it
goes red on an unshrunk "open" shot). The full walk (`--screenshots`
with no `--only`) photographs every state in the table at every profile.
