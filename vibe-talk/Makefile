# vibe-talk's own validation, runnable from this directory alone.
#
# WHY THIS EXISTS SEPARATELY. vibe-talk is a service, not one of the paired command-line tools, and
# it lives outside the `rs/` workspace with its own lockfile. The repository's `make check` and
# `make test` do not see it at all. The repository-wide `make validate` does -- it selects the
# vibe-talk group when vibe-talk changed -- but it is run from the root and reasons about the whole
# tree, and the loop you actually work in is "I changed one file in here, is this tool still good?"
#
# So: `make validate` at the root is the repository's boxed gate, and `make validate` HERE is this
# tool's lightweight unboxed gate. The root includes `tests.dag.yaml` directly so vibe-talk shares
# one scheduler and browser cap with the other repository fragments.

.PHONY: validate validate-boxed dag-check fmt check scripts-check contract contract-check typecheck test page pwa-installability enter-to-send-browser pairing-link-browser offline-cache-browser reactions-browser system-bots-browser edited-mark-browser rich-headers-browser read-aloud-browser read-aloud-blocks-browser post-confirm-browser approval-card-browser voice-call-drops-browser send-order-browser view-switch-browser read-modes-browser keyboard-browser search-chord-browser two-column-browser desktop-layout-browser bookmark-browser reply-continuity-browser reply-in-column-browser reply-marks-read-browser done-sync-browser voice-read-acceptance-self-test android-speech-self-test live-hint-adapter-self-test container-smoke filter-place-browser search-dates-browser voice-composer-browser composer-grow-browser thread-button-browser done-feedback-browser elision-browser clean thread-card-focus-browser thread-card-header-browser thread-line-browser empty-messages-browser all-thread-strip-browser scroll-stability-browser copy-message-browser fold-steps-browser back-gesture-browser live-pill-browser stale-live-read-browser provider-groups-browser voice-reconnect-browser live-relay-check-self-test live-relay-check channel-search-browser column-overscroll-browser

# The whole gate for this tool, cheapest and most likely to fail first.
#
# UNBOXED, and deliberately the default: it needs Rust, Node, and the repository's Python typing
# tools, but no cgroup delegation. `validate-boxed` is the bounded version where cgroups exist.
validate: fmt check scripts-check contract-check typecheck test dag-check voice-read-acceptance-self-test android-speech-self-test live-hint-adapter-self-test live-relay-check-self-test

# The complete portable suite, run through dagrun: per-step wall and CPU timeouts, memory caps,
# and process-tree teardown inside nested cgroups. Container-runtime and alternate-toolchain
# checks remain focused CI signals because they require a different execution environment.
#
# PREFER THIS LOCALLY. It is the only path that bounds what the screenshot harness starts -- the
# teardown is setsid-proof, so a browser or a throwaway server is reaped with the step whether or
# not the script's own trap fired. One that was not once ran for six days holding a port.
#
# It also adds PWA installability, offline-cache, and screenshot browser suites, which plain
# `validate` leaves out because they need a browser.
validate-boxed:
	cd .. && ./common/bin/dagrun run --dag vibe-talk/tests.dag.yaml

# `./common/bin/dagrun`, not a bare `dagrun` and not `./bin/dagrun`. This repository ships the tool
# it uses, so depending on the caller having installed one would fail for a reason that has nothing
# to do with this tool -- and `./bin/` is GITIGNORED, existing only after `./setup` has run, so
# pointing at it failed on a fresh clone for a reason that had nothing to do with it either.
#
# The graph itself, checked without running anything. Offline and instant.
#
# This used to prove almost nothing, and said so: `dagrun list` parsed the document and printed it,
# accepting an unknown field, a duplicate step tag, a missing dependency and a cycle -- the last of
# which CRASHED the runner rather than being refused. `#96 dagrun-loader-validation` moved all of
# those into the loader, so every inspection subcommand gets them: this now refuses a closed-schema
# violation, a duplicate tag, a missing dependency, a named cycle, and a demand above its cap, at
# exit 2, having executed nothing.
dag-check:
	cd .. && ./common/bin/dagrun list --dag vibe-talk/tests.dag.yaml >/dev/null

fmt:
	cargo fmt --all -- --check

check:
	cargo clippy --all-targets --locked -- -D warnings

# These are deployed operator tools, not incidental repository helpers. Keep their strict typing
# in this tool's own gate so changing one does not need the unrelated root workspaces merely to
# reach a mypy invocation.
scripts-check:
	cd .. && python3 -m mypy --config-file py/pyproject.toml --strict \
		--disallow-any-explicit --disallow-any-decorated vibe-talk
	cd .. && python3 scripts/check_no_any.py vibe-talk

# THE WIRE CONTRACT. `src/contract.rs` names the JSON the server and the page exchange; the files
# under contract/ and web/contract.* are generated from it and checked in, so a change to a wire
# type arrives as a reviewable diff. `contract` regenerates all of them: the schema and samples
# from the Rust types, then the browser's validators and declarations from the schema, then every
# contract test against the result. The browser-validator test is skipped while rewriting: it
# would judge the OLD validators against the NEW samples, and stop the recipe before regenerating.
# `contract-check` fails, naming the stale file, when the browser files no longer match the
# schema; `cargo test --test contract` does the same for the schema against the Rust types.
#
# Both, and `typecheck`, need the generator's pinned tools: `npm ci --prefix contract` once.
# Serving the page never does -- web/contract.js is plain checked-in JavaScript.
contract:
	VIBE_TALK_UPDATE_CONTRACT=1 cargo test --locked --test contract -- --skip the_browser_validators
	node contract/generate.mjs
	cargo test --locked --test contract

contract-check:
	node contract/generate.mjs --check

# TypeScript checks web/voice.js as JavaScript (checkJs, noEmit) against the generated
# declarations. Nothing is compiled or bundled; see tsconfig.json.
typecheck:
	contract/node_modules/.bin/tsc -p tsconfig.json

# `cargo test` runs the page's suites through `node --test` as well as the Rust tests, so this is
# not only the Rust half. Those suites FAIL rather than skip when node is missing, which is what
# stops a page regression from passing silently on a machine without a node toolchain.
test:
	cargo test --locked

# The page suites on their own, for the edit-run loop. Seconds rather than a full cargo test.
#
# EVERY suite, by glob, not a named file: the page's own suite and the contract's validators are
# two, and naming one of them is how a third comes to exist without ever being run.
page:
	node --test tests/js/*.test.mjs

# A real current Chromium, speaking CDP. This validates the installability decision the DOM-only
# page suite cannot make, at / and from the app's old address /voice (`#302 root-is-the-app`): a
# 302 keeping query and fragment, the same app id, inside the scope. It runs the release binary on a
# loopback secure context by default, so it builds that first; set VIBE_TALK_CHROMIUM to inspect
# with a particular Chrome executable, or pass --url directly to the script for a deployed server.
# It deliberately states that this is not physical-device launcher proof.
pwa-installability:
	cargo build --release --locked
	python3 tests/pwa_installability.py

# `#225 enter-to-send` in a real Chromium at a desk's size: real keys in the reply box. By default
# Shift+Enter is a new line and Enter posts the reply; with "Enter to send" off in Settings (its hint
# naming the key that sends, kept across a real reload), Enter is a new line and Ctrl+Enter posts.
# Pass WEB_ROOT=dir to run it against another copy of the page.
enter-to-send-browser:
	python3 tests/enter_to_send_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))

# `#369 pairing-link` in a real Chromium at a phone's size: a pairing link signs a fresh browser in
# once, its code out of the address before any request; the same link in a second browser is refused.
# Pass WEB_ROOT=dir to run it against another copy of the page.
pairing-link-browser:
	python3 tests/pairing_link_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))

# `#18 offline-message-cache` in a real Chromium at a phone's size: localStorage across a real
# reload, a real fetch failure, the Cache Storage and service-worker registries, and the pill's
# layout against the view tabs -- what the DOM-only suite cannot observe. Loopback fake API only.
# Pass SCREENSHOTS=dir to keep one PNG per step.
offline-cache-browser:
	python3 tests/offline_cache_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#219 emoji-reactions` in a real Chromium: every reaction strip laid out under its row's text,
# clear of the row's controls and legible, at 412px, 360px and a desk, dark and light. Pass
# SCREENSHOTS=dir to keep PNGs.
reactions-browser:
	python3 tests/reactions_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#323 slack-system-bots` in a real Chromium: a Slack welcome bot drawn as another bot, named and
# labelled, set in and coloured as a colleague is, and a named coding agent drawn as the agent beside
# it, at a phone and a desk, dark and light. Pass SCREENSHOTS=dir to keep PNGs.
system-bots-browser:
	python3 tests/system_bots_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#320 slack-edited-mark` in a real Chromium: the "(edited)" after an edited row's time, on one line
# inside the meta line, after the time, clear of the row's buttons, in the time's ink and size, at two
# phone widths and a desk, in both schemes. Pass SCREENSHOTS=dir to keep one PNG per size and scheme.
edited-mark-browser:
	python3 tests/edited_mark_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#242 rich-headers` in a real Chromium: each agent's tag, title and time drawn whole in the row's
# header where it fits and as its first line where it does not, the words they restate hidden and
# none lost, at 412px and a desk (and two columns), and the Settings switch drawing every first line
# as written, kept across a reload. Pass SCREENSHOTS=dir to keep PNGs.
rich-headers-browser:
	python3 tests/rich_headers_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#7 read-aloud-latency` in a real Chromium: tap to audible against audio streamed at speaking
# pace, a switch and a stop closing the response, and only a read heard to its end archived. Its
# negative control is the same tap through `<audio src>`, which must take seconds. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
read-aloud-browser:
	python3 tests/read_aloud_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#376 read-aloud-blocks` in a real Chromium: a long message read block by block -- lit, counted,
# jumped to by a tap, moved by Previous, Next and the media keys -- against a one-session voice at
# speaking pace, with every block boundary measured on the audio clock. Its negative control is a
# voice whose first audio is slow, which must show as a gap. Pass WEB_ROOT=dir to run it against
# another copy of the page, and SCREENSHOTS=dir to keep PNGs.
read-aloud-blocks-browser:
	python3 tests/read_aloud_blocks_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#49 post-confirmation-card-hold` in a real Chromium: a real touch on Send the moment a first
# post-confirmation card appears commits nothing, and Send enables itself after the hold. Pass
# WEB_ROOT=dir to run it against another copy of the page.
post-confirm-browser:
	python3 tests/post_confirm_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))

# `#268 voice-call-drops` in a real Chromium: a spoken call on the fake microphone replaying the
# voice bridge's event order -- no microphone frame inside the greeting's hand-off, a "Yes." whose
# final transcript arrives after the agent's reply committing the card once and never laid out as a
# turn of its own (`#269 approval-card-inline`: it is drawn into the frozen card), the page's notice
# waiting for that turn, the server's close said and recorded, and a hang-up sending one goodbye;
# then (`#330 voice-long-turn-drop`, `#337 voice-relay-silent`) a long answer shown as it arrives and
# heard out before a message from another watched channel goes, a typed cut that keeps the cut
# answer's words, and a turn heard without text saying so.
# Pass WEB_ROOT=dir to run it against another copy of the page.
voice-call-drops-browser:
	python3 tests/voice_call_drops_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))

# `#352 voice-reconnect-on-bridge-error` in a real Chromium: a fake vibe-talk-v1 bridge loses a turn
# after an interrupt and a relayed message, and sends "turn N never started after an interrupt;
# reconnect". The call stays up on a second session -- the deployed shape (a greeting, no replay) with
# the bridge closing the lost socket at once, and a bridge that lists replay, handed this call's record
# first -- the relayed message goes again once, the microphone reaches only the new session, and the
# page says so; a second lost turn within the minute ends the call as reconnect_limit, and any other
# error still ends it as error_frame. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep PNGs.
voice-reconnect-browser:
	python3 tests/voice_reconnect_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#277 send-order` in a real Chromium against the real server binary, which posts through its real
# provider client to a mock Google Chat bridge on loopback that fails A at once: B waits behind A,
# saying so, and the bridge never sees it; A goes again under the same nonce and lands, then B;
# across a real reload a message still waits behind the one before it, sending nothing, until Retry
# lands both in order; Dismiss on a failing message lets the next go; a slow bridge is given one post
# at a time, in order; and a post whose caller hangs up mid-post still lands before the next one.
# Builds the release binary first, because web/ is compiled into it. Pass SCREENSHOTS=dir to keep a
# PNG of each waiting state.
send-order-browser:
	cargo build --release --locked
	python3 tests/send_order_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#269 approval-card-inline` in a real Chromium at 393x852, 360x780 and 375x667: the send card laid
# out as the next message -- in the list on the call's view and the channel, docked on Settings --
# with Send's centre at least 0.62 of the way down the screen, uncovered, wholly on screen with where
# the draft goes, the box's own Send out of the way, the card at the owner's own inset, and both
# buttons held when it appears; then the frozen card it becomes, filed after its commit and restored
# by a reload (`#270 voice-transcript-markers`); and at 1280x800 in two columns, wholly above a reply
# open at the foot of the left column (`#277 reply-in-column`), its Send outlined beside a reply in
# either column; and on the three phones, the line above a hosted call sent a 412-word recap saying
# so (`#270`), and the card coming and going on the channel leaving a reader up the list on their row
# and one at the foot there (`#280 scroll-stability`); and at 393x852 and 1280x800 docked under the
# thread list, through Cmd/Ctrl+3 and +4 pressed on its own buttons, which post nothing (`#281
# view-keys`), and back from it with the channel's reader where they were, at the foot of All or on
# their row up All or Main, card or no card, and a draft arriving under it leaving a reader up Main on
# their row (`#280`); and at 1280x800 a reader up the list's cards on their card again by Ctrl+4 and
# Ctrl+3, though two columns, the card gone from under their cards, put them at the foot there; and at
# 393x852 and 1280x800 a reader at the foot of Main or All still there once the box takes back its room
# a second after Send, the sent message drawn by a live read in that second and with the browser's
# scroll anchoring off, and after a draft opened by More is settled elsewhere and the box takes the
# focus; and at 393x852 a reader who sees only a long draft opened by More finding it
# where it stood through the thread list and back and across Main and All (`#280`); and at 1280x800,
# the opened draft drawn taller in two columns' left column, a reader at the foot of Main or All at
# the foot through Ctrl+4 and back with Send tappable, one up either on their row, and one near the
# foot whom the change could not keep there on their row again on the way back; and at 393x852,
# 360x780 and 375x667 a reader who saw only that draft's card, past the slack of the foot, at the foot
# with no jump a second after Send or the draft settled elsewhere, in Main, All and the call's view, and
# at 393x852 one who saw only it as the thread list docked it, on the way back once it had gone
# (`#280`); and at 393x852 and 1280x800 a reader of the thread list to whom a draft came while Settings
# or Help was up, back at the foot of its cards over the docked card if they left from it (`#336
# draft-return-thread-list`). Pass WEB_ROOT=dir for another copy of the page, SCREENSHOTS=dir to keep a
# PNG of each placement.
approval-card-browser:
	python3 tests/approval_card_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#241 voice-composer` in a real Chromium at 412x915 and 1280x800: the call's message box and the
# view switch on screen and uncovered while the box has the focus -- at idle, in a typed
# conversation, in a call, and on the phone with the on-screen keyboard's share of the screen taken
# away -- focus and typing opening nothing, and the first send opening one typed conversation. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
voice-composer-browser:
	python3 tests/voice_composer_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#340 composer-grow-box` in a real Chromium: the Grow checkbox left of Send, and with it ticked a real
# long paste growing the message box to 60% of the window with Send on screen, at 1280x800, 1920x1080,
# 1280x600 and in two columns, with field-sizing turned off too, a reader at the foot kept there while
# typing and when Grow is ticked after a paste, one a little above it keeping the checkbox under the
# pointer, also by a click that focuses no checkbox as in Safari and Firefox on a Mac, one up the list
# not moved, the call's box with its recap line shown above it, and a 393x852 phone unchanged. Pass
# WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state, Grow off and on.
composer-grow-browser:
	python3 tests/composer_grow_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#220 view-switch-instant` in a real Chromium: All to Main and back with every timeline request
# intercepted -- no read, never an empty list or the loading line, the reader's message kept -- All
# never read drawn at once from held rows and read once behind them, and, paged, Main drawn from All
# keeping its way back and All walked back to the reader's message before it is drawn. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
view-switch-browser:
	python3 tests/view_switch_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#221 read-modes` in a real Chromium at 412x915 and 1280x800: entering a thread lands on its first
# unread message, unfolded, under the pill, in each read mode, and Collapse read's "… N read
# messages …" line is a 44px target a real tap opens without moving the reader; then, `#222
# unread-replies`, Main under Hide read keeps the read question for its unread replies, says "4 unread"
# beside "52 replies", and a real tap on the count lands on the first of them. Pass WEB_ROOT=dir to
# run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each step.
read-modes-browser:
	python3 tests/read_modes_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#224 keyboard-shortcuts` in a real Chromium at 1280x800, the keys pressed through CDP
# Input.dispatchKeyEvent so the browser's own defaults run: PgUp, PgDn, Space, Home and End move the
# list on screen with the focus where the page put it on load and after a view switch; "/" and Ctrl+S
# open the search (the browser's Save page refused); "," and Ctrl+, open Settings and Escape comes
# back; in the message box PgUp pages the list and Ctrl+Home/End jump it; then the keymap: j, k and the
# arrows move a visible focus ring, e marks Done and moves on, PgDn brings the ring to the page, Shift+E
# marks one opened message Done, asks the chat service once to mark it read, and folds it with nothing
# on screen moving (`#295 shift-e-read-collapse`), Enter folds, and -> enters a thread that <- and Esc
# leave; Ctrl+K and a channel chosen give the keys back to the list, and Esc leaves the picker;
# `#281 view-keys`: Ctrl+1 to Ctrl+4, and Cmd on an emulated Mac,
# choose Main, All, the thread list and two columns from the message box, the search and a card, the
# browser's default refused, and Esc on the thread list goes back to what it was chosen from. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each step.
keyboard-browser:
	python3 tests/keyboard_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#344 search-shortcut` in a real Chromium at 1280x800, the keys pressed through CDP: Ctrl+/ opens the
# search from the message box holding a draft (untouched), the pace popover and a ⋯ menu (closed first),
# the channel picker, Settings, Help, the right column, past a listener that stops the key, the Reply
# screen (its draft kept) and with single keys off; Ctrl+Shift+? opens Help; a German Shift+7 "/", an
# AltGr "/" and a held chord; Cmd on an emulated Mac. Pass WEB_ROOT=dir to run it against another copy
# of the page, and SCREENSHOTS=dir to keep a PNG of each step.
search-chord-browser:
	python3 tests/search_chord_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#355 provider-grouped-channels` in a real Chromium at 1280x800 and a 375x812 phone: with two chat
# services, the bar's channel picker holds one <optgroup> per service in configuration order; Alt+Down
# and Alt+Up, pressed through CDP, step across the groups in the order shown and stop at the ends; a
# reload restores the channel inside its group; Ctrl+K and a choice read that channel; on the phone the
# picker stays inside the window. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state.
provider-groups-browser:
	python3 tests/provider_groups_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#371 channel-search-hash` in a real Chromium at 1280x800: with one Slack service, the bar's picker
# lists `#team-builds` as `team-builds`, so typing t, e, a, m into the focused picker (keys through CDP)
# chooses it; Browse channels counts the seconds of a slow first read, then finds `#team-builds-archive`
# for `Builds` and `#eng-ops` for `#ops`. Pass WEB_ROOT=dir to run it against another copy of the page,
# and SCREENSHOTS=dir to keep a PNG of each state.
channel-search-browser:
	python3 tests/channel_search_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#229 desktop-two-column` in a real Chromium at 1440x900 and 1280x800: the layout button draws Main
# and the right column side by side inside the window, the view picker saying so; `#281 view-keys`: the
# thread list chosen from the picker is that column across the window, and real Ctrl+2 and Ctrl+4
# leave and come back to two columns; the right column's cards are one height, newest activity first,
# the long one cut to three lines; a card's replies button and a Main message's Thread(N) select a
# thread there under its name, each column scrolls alone under the wheel, the X goes back to the cards,
# and a window narrowed to 900px is one column in its old view, two again when wide. Pass WEB_ROOT=dir
# to run it against another copy of the page, and SCREENSHOTS=dir to keep the list and thread states as
# PNGs.
two-column-browser:
	python3 tests/two_column_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#372 settings-header-layout` and `#373 two-column-full-width` in a real Chromium. Settings at
# 1280x800, 1920x1080 and a 393x852 phone, under a host too long for the phone's line: the host one
# quiet line, its port whole, Expand all and Open help on its row (under it on the phone), Tab in that
# order, Expand all turning to Fold all in place. Two columns at 1920x1080 and 2560x1080 over the
# agent's long replies and the owner's answer with no threads: every row of Main but the owner's across
# its column to the gutter by the divider, the owner's kept partial, the glass over no row, the right
# column's heading and empty line across it, one column still at its reading width; and a thread open
# on the right, its rows across that column. Pass WEB_ROOT=dir to run it against another copy of the
# page, and SCREENSHOTS=dir to keep each state as a PNG.
desktop-layout-browser:
	python3 tests/desktop_layout_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#277 thread-card-focus` in a real Chromium, at 1280x800 over the right column's cards with a mouse and
# the keys, and on a 412x915 phone over the Threads list with a finger: every card contracted, the one
# under the mouse too; a click or a tap highlights one, shown whole and not entered, and every other
# card's edge and every other link turns the page's grey — by the colour Chromium computes and by the
# pixels a screenshot holds there — the highlighted card's staying in its colour; its replies button
# enters its thread, <- or Back comes out to it still highlighted, Esc puts the highlight out, and -> enters
# the card the keys are on. `#281 view-keys`: then the phone's thread list, chosen in the view picker, the
# same cards across the window: the channel's highlight on them, the others grey, each replies button a
# thumb high, a tap highlighting and showing one whole, its replies button opening the thread across the
# window and Back coming to the list. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state.
thread-card-focus-browser:
	python3 tests/thread_card_focus_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#296 copy-highlighted-message` in a real Chromium, the keys pressed through CDP and the clipboard read
# back with `clipboard-read` granted: at 1280x800, Ctrl+C with nothing highlighted copies nothing; on the
# folded long question it copies all of it as written, with a text/html flavour, and moves nothing; in the
# message box, and with words selected, the browser's copy as ever; y copies the message, and a thread
# list card copies its first message; in a tab emulating a Mac, Cmd+C with the Edit menu's "copy" command
# and without it; on a 412x915 phone with a hardware keyboard, a tap's silent selection copies nothing and
# j's highlight does. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each state.
copy-message-browser:
	python3 tests/copy_message_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#304 back-gesture-in-app` in a real Chromium at a 412x915 Android phone in standalone mode: the browser's
# Back -- what the phone's back gesture sends -- closes a thread in the same document with the reader's place
# kept, the thread list and a thread opened from it are two entries closed in turn by history.back() and the
# browser's Back, the ⋯ menu over a thread is an entry the browser's Back closes alone, the on-screen Back goes
# back through its entry so the browser's Back at the top leaves the app, and a page reloaded in a thread opened
# from the list stands on the earlier load's entries, so the first Back, before any touch, closes the thread
# alone. With the app as the tab's first entry, as in an installed app, no entry is left under it after a
# reload; at a 1440x900 desk in two columns the browser's Back with a ⋯ menu open closes the menu, not the
# thread on the right. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each state.
back-gesture-browser:
	python3 tests/back_gesture_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#324 live-pill-covers-row` in a real Chromium at a 412x915 phone with touch, in both schemes, the pill saying
# "Live · updated": in Main the unread chip scrolled onto the pill's line is clipped by the list's top edge, not
# covered, and in Main and in a thread scrolled through nine places nothing of the list is under the pill or the
# search glass, by a tap through the pill and by the pixels there; a thread entered from its root lands with the
# root's date rule below the floating line. Its controls put the old room, padding inside the scroller, back and
# find the list under the pill. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir
# to keep a PNG of each state.
live-pill-browser:
	python3 tests/live_pill_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#298 thread-card-header` in a real Chromium at 1280x800 with a mouse, in both schemes: a card's replies
# button opens its thread on the right under its card at the top of the column — the card's words, place,
# height and colour, by computed colour and pixels, the X on it — joined to the thread's root on Main by
# one line, clipped at Main's top while the root is above it, that the mouse resting on the card glides
# Main along and the mouse crossing it does not; a long question's card the same height, opened over the
# list with the list unmoved to the pixel, and Esc closing it, then the thread; the card moving up from its
# place in the list, frame by frame, with nothing under it moving; the same card heading a thread opened
# from the thread list (real Ctrl+3) across the window, at the desk and on a 412x915 phone by touch; the
# card opened on the right, then Ctrl+1 and a thread from the picker, the first Esc going back to Main;
# nothing moving under reduced motion; and a redraw of Main leaving the keys in the right column, so Esc
# still closes its thread (`#284 redraw-keeps-pane`). Pass WEB_ROOT=dir to run it against another copy of
# the page, and SCREENSHOTS=dir to keep a PNG of each state.
thread-card-header-browser:
	python3 tests/thread_card_header_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#385 thread-line-overlap` in a real Chromium at 1000x800, 1280x800 (both schemes) and 1920x1080 with a
# mouse, and 393x852 by touch: the line from a thread's root to its card on the right runs down a lane at
# least 4px left of every row of the thread, whole and clipped at Main's top, and the owner's rows stay
# partial width; on the phone no line is drawn. Pass WEB_ROOT=dir to run it against another copy of the
# page, and SCREENSHOTS=dir to keep a PNG of each state.
thread-line-browser:
	python3 tests/thread_line_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#386 hide-empty-messages` in a real Chromium at 1280x800 (mouse) and 393x852 (touch), in both schemes:
# with the Settings switch on, its default, no empty message is drawn or counted, in All, Main or a thread
# on the right, while a file alone and a reaction alone are drawn; off, every empty message is back, kept
# across a reload. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each state.
empty-messages-browser:
	python3 tests/empty_messages_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#299 all-thread-strip` in a real Chromium at 1280x800 (mouse) and 412x915 (touch), in both schemes: in
# All, a strip on every row of an answered thread and on no other row, its bar at least 5px (6px under a
# finger) in its Thread(N)'s colour, its hit box 16px (22px) and the row's full height, every row's words
# at one inset, a read reply's strip still coloured, by computed colour and pixels; a click (a tap) on a
# strip: nothing scrolls or moves, still All, the rest at 0.3 and drained with grey strips; a live reply of
# the thread arriving in it and a live message of no thread dimmed; a second click, Esc, another strip and
# t; Thread(N) and Back keeping the selection; a first reply giving its root a strip without moving its
# words; a finger drawn sideways from a strip archiving nothing. Pass WEB_ROOT=dir to run it against
# another copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
all-thread-strip-browser:
	python3 tests/all_thread_strip_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#310 stale-live-read-flicker` in a real Chromium at 1280x800: in All on its newest line, two live
# messages, the second pushed once the first one's read has reached the server, each live read answered
# 300 ms late as the server had the channel when it arrived; the second message's row drawn and laid out
# in every animation frame from its first until both reads have landed and 30 frames more. Its negative
# control runs the same walk against a copy of the page with the fix taken out, which must miss the row
# in some frame. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each state.
stale-live-read-browser:
	python3 tests/stale_live_read_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#280 scroll-stability` in a real Chromium at 1280x800 in two columns (mouse) and 412x915 in one (touch):
# the reader scrolled up in Main stays on the same row, within a pixel, while a message and a reply arrive
# live, a refresh redraws the list with a message above them grown, a read sent while they were on the
# newest line lands after they scrolled up (the owner's jump), summaries arrive, and the focus moves, and
# at the desk while the mouse crosses the cards to the dock or the top bar and rests, and ] is pressed on
# its own, each sampled every frame for 2.5 s; a reader on the newest line follows a live message to the foot; the list of threads and a thread open at
# their foot and stay there, older replies coming in under a reader at a thread's head; after a
# reload the read behind the saved rows leaves the reader where they scrolled; `#281 view-keys`: the view
# keys show each view where it is read and nothing moves it for 2.5 s after, the thread list opening at its
# foot from two columns, Main and All, Main coming back from it where the reader left it, a thread
# opened from it opening at its foot, and a reload in it opening it at its foot with the desk's dock on two
# rows; and, last, a card highlighted by a click, the mouse going on over another card to the dock, leaves
# Main still. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a
# PNG of each stage.
scroll-stability-browser:
	python3 tests/scroll_stability_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#338 column-overscroll` in a real Chromium at 1280x800 and 1920x1080, in two columns with a mouse: at a
# desk the right column scrolls on past its foot into blank room until the last card's top is at half the
# column's height -- by the wheel after it has rested at the foot, by PgDn and Space, and by a fresh down
# arrow on the last card -- and back up past the foot, End and a search close it. The mouse resting on the
# last card, a press that shows it whole or puts it out, and a reply arriving in the room move nothing; a
# fling of the wheel and a held key stop on the foot; and the thread list on its own (Ctrl+3) does the same.
# Six negative controls follow at 1920x1080 (NO_CONTROLS=1 skips them). Pass WEB_ROOT=dir to run it against
# another copy of the page, and SCREENSHOTS=dir to keep a PNG of each stage.
column-overscroll-browser:
	python3 tests/column_overscroll_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS)) $(if $(NO_CONTROLS),--no-controls)

# `#240 reading-bookmark` in a real Chromium at 1280x800 (mouse and keys) and 412x915 (touch): the
# bookmark set on a message (Shift+B, or the row's menu) is a line in the accent drawn in the room
# between that row and the one above; scrolled away, the Bookmark chip points to it, directly right of
# Newest on the bottom line; b or the chip puts the line at the top of the list; a reply goes to the
# newest message and b comes back; and a Done on the message under the line slides it to the next, on
# the server too. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each step.
bookmark-browser:
	python3 tests/bookmark_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#243 filter-keeps-selection` in a real Chromium at 1280x800 and 412x915, the owner's own sequence:
# search, Pinned, a pin chosen with the mouse or a finger, the filter cleared -- the pin's row stands in
# the channel's list where it stood in the Pinned list, within a pixel, and stays there; then an old pin,
# two pages beyond what is loaded, walked back to under a "Loading earlier messages…" line and put where
# it stood. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a
# PNG of each stage.
filter-place-browser:
	python3 tests/filter_place_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#249 search-highlight`, `#250 date-separators` and `#251 author-filter` in a real Chromium at
# 1280x800 (mouse) and 412x915 (touch), over a week of a channel read at a fixed noon: a rule above the
# first message of each day, the bookmark's line nearer the message than its day's; a search's hits lit,
# its gaps said with the days they reach, each line as wide as the list and touching no row; and the
# author filter showing only the owner's prompts with the one chosen left where it stood. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each stage.
search-dates-browser:
	python3 tests/search_dates_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#247 reply-continuity` in a real Chromium at 1280x800 (mouse) and 412x915 (touch), at 80%, 100% and
# 130% text: Reply on a message in the middle of the list, just under where the title bar goes, half
# under the list's top edge, low on the screen, a reply in its gutter, and one taller than the screen.
# The words are the list's size and width, the message's top stays where the row's was (within 1px) or
# at the top, the box comes up directly under it, a low one moves up only as far as shows it, and
# Cancel comes back to the pixel; the reply screen is tinted in the dark scheme and the light. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each.
reply-continuity-browser:
	python3 tests/reply_continuity_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#277 reply-in-column` in a real Chromium at 1280x800, in two columns: Reply and Quote reply (words
# dragged over with the mouse) pressed in each column open the reply box at the foot of that column, both
# columns staying where they were to the pixel and the message answered edged and unmoved; Escape, Cancel,
# Send and Enter put the box back and the column where it was, the keys on its list, a sent reply posted
# where the reply screen posts it; a message low in a column moves up only as far as shows it over the
# box; a window narrowed to one column hands an open reply to the reply screen with its words, and
# `#314 reply-focus-flake` with the focus in its box even where the page's frame comes after the browser
# has taken the focus off it (the page's frames held back, in each column); `#281
# view-keys`: real Ctrl+4 in an open reply leaves it, and Ctrl+2 and Ctrl+3 show All and the thread list
# with the reply put down, its words kept; and one column is the reply screen, as it was. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each
# open reply.
reply-in-column-browser:
	python3 tests/reply_in_column_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#300 reply-marks-read` in a real Chromium at 1280x800, in two columns: a Reply docked in either column
# and the first reply typed in a new thread's box mark what they answer read — Done on the server, the
# provider asked `after_reply` after it, the row drawn read, in both columns for the thread's root — and
# a later message in an answered thread's box marks nothing. Pass WEB_ROOT=dir to run it against another
# copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
reply-marks-read-browser:
	python3 tests/reply_marks_read_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#306 done-stale-across-views` and `#311 sent-read-waits-for-done` in a real Chromium: on a 412x915
# phone, a Done tapped in All drawn in Main, held from earlier, and an Undo in Main drawn in All, from the
# first frame each view shows and with no read; at 1280x800 in two columns, a Reply sent while the Done of
# the message it answers is held on the server asks the provider nothing until that Done is saved, and
# nothing once it is refused. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each stage.
done-sync-browser:
	python3 tests/done_sync_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#252 thread-button-state` in a real Chromium at 412x915 (touch) and 1280x800 (mouse), in the dark
# and light schemes: a new message's Thread tag, whose thread nobody has answered, is drawn as Reply
# is and an answered root's Thread(N) in its thread's colour; the grey tag opens the empty thread
# with its box focused, the first reply is posted into the message's thread, and the tag turns
# coloured; at the desk, the same on the right in two columns. Pass WEB_ROOT=dir to run it against
# another copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
thread-button-browser:
	python3 tests/thread_button_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#377 compact-collapsed-rows` in a real Chromium at 393x852 (touch) and 1280x800 (mouse), in All: a
# collapsed row's header is one line -- the name cut with an ellipsis where it must be, the time, a
# Thread tag the size of Reply in its thread's colour, and ⋯ -- with Reply and Done (or Unarchive) only
# once the row is opened, every control then together at the right; opened by a tap and by Enter,
# folded again, and swiped Done while collapsed. At the desk a wide list keeps every control on one
# line, in one column and in two at 1280x800, and two columns at 1024x768 follow the phone's rule.
# Pass WEB_ROOT=dir to run it against another copy of the page, SCREENSHOTS=dir to keep a PNG of each
# state, and REPORT=1 to print every header measured and go on past a failed check.
.PHONY: compact-rows-browser
compact-rows-browser:
	python3 tests/compact_rows_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS)) $(if $(REPORT),--report)

# `#245 done-feedback` in a real Chromium: at 1280x800 in both schemes, a selected unread message ringed
# solid in the accent and a selected read one dashed in grey on the same lit surface, each at 3:1 or
# more against it; e draws the moment on the row it left and lets it go; the Undo chip names the
# message, and an Undo inside the hold takes the ring away at once; reduced motion gets the look with
# no animation; at 412x915 and 1280x800 in both schemes, the pictures; on a 360px and a 412px phone the
# Undo chip says only "Undo", taking no line, and at 1280 it names the message on one line; and at all
# three, walked up from the end of the list through the message box, no chip or status pill meets the
# box or its Send. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each step.
done-feedback-browser:
	python3 tests/done_feedback_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#244 elision-word-count` in a real Chromium, in seven layouts (phones at 360 and 412px, at 80% and
# 150% text, a desk, the widest reading column, two columns): every folded row's one label claims no
# more than its clamp really hides, by the rects of its characters; Chinese and a bare address are
# counted in characters; a table the clamp draws whole says nothing, and a table over prose, a code
# block and a list of short items say no more than they hide; a rich header's row holds with the switch
# on and off. Pass WEB_ROOT=dir to run
# it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each layout.
elision-browser:
	python3 tests/elision_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#301 semicolon-expand-cycle` in a real Chromium at a 1280x800 desk: ; four times and : three times step
# the long messages round Expanded all, Expanded unread, collapsed read, and Collapsed all, judged by whether
# each message's text overflows its box, with the status line read; the selected message, sampled at every
# frame after each press, never moves by more than a pixel, at the newest line too; in two columns the keys
# on the right step the right column's thread alone, its reader at its foot held, and ; in Main is Main's
# own; Ctrl+;, Cmd+; and Ctrl+: step from the channel's message box at the list's foot, where a typed ;
# steps nothing, and Ctrl+; in the right column's box steps that column alone, each chord holding the box
# it was pressed in where it is, shown whole at every frame. Pass WEB_ROOT=dir to run it against another
# copy of the page, and SCREENSHOTS=dir to keep a PNG of each step.
fold-steps-browser:
	python3 tests/fold_steps_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# The live #47 read-aloud-overlong check is opt-in and may consume provider time. Its transcript
# reconciliation, PCM-duration metric, re-answer separation, privacy boundaries, threshold edges,
# interruption rejection, and WAV writer are deterministic offline controls, so keep those in the
# ordinary gate.
voice-read-acceptance-self-test:
	python3 scripts/voice-read-acceptance.py --self-test

# The physical-device run is opt-in, but its signal/language evaluator has offline negative
# controls. Keep those controls in the ordinary gate so a broken harness cannot silently bless a
# later Android recording.
android-speech-self-test:
	python3 scripts/android-device-speech.py --self-test

# A live hint carries no chat content, and its adapter owns a long-running child plus retries.
# Keep the offline HTTP, framing, privacy, and process-cleanup controls in the ordinary gate so a
# deployment does not have to discover a broken supervisor against a real provider event stream.
live-hint-adapter-self-test:
	python3 scripts/live-hint-adapter.py --self-test

# `#354 live-relay-check`: the verdict logic of the opt-in live check of relays during a call, on
# synthetic frame logs. No browser, server or bridge.
live-relay-check-self-test:
	python3 scripts/live-relay-check.py --self-test

# The live check itself, against the bridge you name: never part of an automated suite, since it
# makes real calls. BRIDGE=ws://... or wss://... (tools= is refused), or BRIDGE=fake for the
# in-process stand-in. The page is compiled into the binary, so it builds that first.
live-relay-check:
	@test -n "$(BRIDGE)" || { echo "BRIDGE=ws://... (or wss://..., or fake) is required" >&2; exit 2; }
	cargo build --release --locked
	python3 scripts/live-relay-check.py --bridge "$(BRIDGE)" $(LIVE_RELAY_ARGS)

# The LAUNCHER -- run.sh, the script the owner types -- as opposed to the image it starts, which
# the `container` CI job covers separately. Not part of `validate` because it needs a container
# engine and a minute of build. Run it before a deploy.
#
# This comment used to claim "CI runs it on every change" while no workflow ran it at all, and
# that claim is why nobody checked: run.sh had shipped inheriting the host's log driver, so on a
# rootless host defaulting to journald `--logs` printed nothing while --status promised retained,
# readable logs. The suite below catches exactly that and had no runner. It has one now, in
# .github/workflows/vibe-talk.yml, so the sentence is true rather than reassuring.
container-smoke:
	./scripts/test-run-sh.sh

clean:
	cargo clean
