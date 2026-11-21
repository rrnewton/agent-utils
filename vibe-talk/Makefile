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

.PHONY: validate validate-boxed dag-check check scripts-check contract-check typecheck test page web-build offline-cache-browser approval-card-browser container-smoke labels-browser whole-threads-browser whole-threads-server-browser clean thread-hue-spread-browser live-relay-check column-overscroll-browser firstrun-browser

# The whole gate for this tool, cheapest and most likely to fail first.
#
# UNBOXED, and deliberately the default: it needs Rust, Node, and the repository's Python typing
# tools, but no cgroup delegation. `validate-boxed` is the bounded version where cgroups exist.
validate: fmt check scripts-check contract-check typecheck test dag-check voice-read-acceptance-self-test android-speech-self-test live-relay-check-self-test screenshots-self-test local-tts-sidecar-self-test

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


check:
	cargo clippy --all-targets --locked -- -D warnings

# These are deployed operator tools, not incidental repository helpers. Keep their strict typing
# in this tool's own gate so changing one does not need the unrelated root workspaces merely to
# reach a mypy invocation.
scripts-check:
	cd .. && python3 -m mypy --config-file py/pyproject.toml --strict \
		--disallow-any-explicit --disallow-any-decorated vibe-talk
	cd .. && python3 scripts/check_no_any.py vibe-talk


contract-check:
	node contract/generate.mjs --check

# TypeScript as a checker only (noEmit): web/voice.js as JavaScript (checkJs) against the generated
# declarations and the modules it calls (tsconfig.json); the page's modules below web/src strict,
# the core alone with no browser or Node declarations (tsconfig.core.json), and all of them with the
# browser's (tsconfig.web.json); the negative type examples, writes to read-only records that must
# not compile (tsconfig.types.json, tests/types). The page build compiles them; see web/build.mjs.
# Then the import directions between them and what each may say (web/architecture.mjs), which the
# page-build suite also holds.
# `#2042`: under the gate's memory box Node sizes its old-space from the cgroup limit
# (~327 MB where tsc needs more), which starves the checker into a GC death spiral;
# the check's heap is therefore said out loud. Keep it under the step's
# rss_baseline_bytes in tests/dag.d/005-contract.yaml.
TSC_NODE_OPTIONS := --max-old-space-size=1536

typecheck:
	NODE_OPTIONS="$(TSC_NODE_OPTIONS)" contract/node_modules/.bin/tsc -p tsconfig.json
	NODE_OPTIONS="$(TSC_NODE_OPTIONS)" contract/node_modules/.bin/tsc -p tsconfig.core.json
	NODE_OPTIONS="$(TSC_NODE_OPTIONS)" contract/node_modules/.bin/tsc -p tsconfig.web.json
	NODE_OPTIONS="$(TSC_NODE_OPTIONS)" contract/node_modules/.bin/tsc -p tsconfig.types.json
	node web/architecture.mjs

# `cargo test` runs the page's suites through `node --test` as well as the Rust tests, so this is
# not only the Rust half. Those suites FAIL rather than skip when node is missing, which is what
# stops a page regression from passing silently on a machine without a node toolchain.
test:
	cargo test --locked

# The page suites on their own, for the edit-run loop. Seconds rather than a full cargo test.
#
# EVERY suite, by glob, not a named file: the page's own suite and the contract's validators are
# two, and naming one of them is how a third comes to exist without ever being run. The second glob
# is the page modules' own suites, one directory down (tests/js/<layer>/<area>.test.mjs).
page:
	node --test tests/js/*.test.mjs tests/js/*/*.test.mjs

# The page build (`#624 voice-page-modularity`): the shell, one minified script and one stylesheet
# under content-hashed names, their source maps and assets.json, into target/web. Every cargo build
# runs the same build (build.rs) and embeds it: the server serves its hashed files at /assets/, while
# its shell still names web/'s files as written. Needs the pinned esbuild: `npm ci --prefix contract`.
web-build:
	node web/build.mjs --out target/web





# `#18 offline-message-cache` in a real Chromium at a phone's size: localStorage across a real
# reload, a real fetch failure, the Cache Storage and service-worker registries, and the pill's
# layout against the view tabs -- what the DOM-only suite cannot observe. Loopback fake API only.
# Pass SCREENSHOTS=dir to keep one PNG per step.
offline-cache-browser:
	python3 tests/offline_cache_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

















# `#431 voice-transcript-expand` in a real Chromium at 393x852 by touch and 1280x800 by mouse: a reply
# streamed over a fake vibe-talk-v1 bridge grows past the fold and stays whole while the list follows
# it; a tap on the card closes it and another opens it; a long reply that arrives whole arrives folded
# and opens on a tap at the newest line; a fold set while the reply is spoken holds through its later
# frames; scrolled up, a card opens where it stands. Pass WEB_ROOT=dir to run it against another copy
# of the page, and SCREENSHOTS=dir to keep a PNG of each state.
.PHONY: voice-expand-browser


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
# PNG of each placement, PART=name to run one of its five parts (the gate runs each as its own step).
approval-card-browser:
	python3 tests/approval_card_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS)) $(if $(PART),--part $(PART))

























# `#564 message-summary-toggle` in a real Chromium at 393x852 (dark and light), 412x915 with touch and
# 1280x800: an unpacked thread whose row holds a quote and a wide code block stays inside the screen,
# the block scrolling in its own box, and a row whose summary failed keeps its Done grey and its
# Unarchive, with only a small muted label. Pass WEB_ROOT=dir to run it against another copy of the
# page, and SCREENSHOTS=dir to keep a PNG of each state.
summarizer-health-browser:
	python3 tests/summarizer_health_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))



# `#414 thread-hue-spread` in a real Chromium at 1280x800: a channel that remembers 24 thread colours
# from an earlier day, handed out as the page before `#414` did, gets eight new threads in a row; the
# remembered keep their colours and each new one is drawn at least 45 degrees of hue from every thread
# within three places of it. Pass WEB_ROOT=dir to run it against another copy of the page (a page from
# before `#414` fails, naming the closest pair), and SCREENSHOTS=dir to keep a PNG of the newest roots.
thread-hue-spread-browser:
	python3 tests/thread_hue_spread_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))













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






# `#1825 vibe-talk-hashtag-labels` in a real Chromium at 412x915 (touch, scale 2.6) and
# 1280x800 (mouse), light and dark, keyboard up on the phone: label chips, the hidden
# hashtag-only line, label: search, Whole threads, the chip tap in place, and
# Settings > Labels in both sorts with the Combine dialog and its (G) rows.
labels-browser:
	python3 tests/labels_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#1852 vibe-talk-search-whole-threads-toggle` in a real Chromium at 412x915 (touch, scale 2.6)
# and 1280x800 (mouse), light and dark, keyboard up on the phone, Discuss open on the phone: the
# Whole-threads toggle icon on the search bar (tooltips on every bar icon, the old chip gone)
# widening word and label: searches to whole threads in All, the thread list and two columns,
# and a reply sent while the filter stands.
whole-threads-browser:
	python3 tests/whole_threads_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))

# `#1864 vibe-talk-whole-threads-no-duplicate-hits` against the real server binary
# (--fake-discord upstream, loopback model stub): a server hit already on screen is
# not drawn again, with the Whole-threads toggle off and on, phone and desk, light
# and dark. Needs `cargo build --release` first.
whole-threads-server-browser:
	python3 tests/whole_threads_server_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))


















# `#377 compact-collapsed-rows` in a real Chromium at 393x852 (touch) and 1280x800 (mouse), in All: a
# collapsed row's header is one line -- the name cut with an ellipsis where it must be, the time, a
# Thread tag the size of Reply in its thread's colour, and ⋯ -- with Reply and Done (or Unarchive) only
# once the row is opened, every control then together at the right; opened by a tap and by Enter,
# folded again, and swiped Done while collapsed. At the desk a wide list keeps every control on one
# line, in one column and in two at 1280x800, and two columns at 1024x768 follow the phone's rule.
# Pass WEB_ROOT=dir to run it against another copy of the page, SCREENSHOTS=dir to keep a PNG of each
# state, and REPORT=1 to print every header measured and go on past a failed check.
.PHONY: discord-split-merge-browser

.PHONY: compact-rows-browser

# `#425 collapsed-lines` in a real Chromium at 1280x800 (keys) and 393x852 (touch): a folded long
# message's body is 3 lines until Ctrl+( and Cmd+( step it to 1 and to 0 -- the header alone, no gap
# where the body was, reactions still drawn -- with the count said and the chord taken from the
# browser, Ctrl+) back up to 6 and no past 8, by bracket and by digit place, never in the message
# box; the Settings slider and its - and + set the same count and a reload keeps it; the row being
# read holds its place as the count changes; the same count folds a thread and rows unpacked under
# their cards, and a raw thread card is the count tall (one line at 0); at 0 a row still opens
# whole, the selection still moves and Reply still opens. Pass WEB_ROOT=dir to run it against
# another copy of the page, SCREENSHOTS=dir to keep a PNG of each state (0, 1, 3 and 6 lines among
# them), and REPORT=1 to print every measure.
.PHONY: collapsed-lines-browser








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

# Per-check targets live with their DAG steps under tests/dag.d/ (#1891):
# one numbered .mk per check beside its .yaml, so adding a check touches one new file.
include $(wildcard tests/dag.d/*.mk)
