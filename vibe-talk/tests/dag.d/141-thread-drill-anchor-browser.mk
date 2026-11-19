.PHONY: thread-drill-anchor-browser
# `#1818 vibe-talk-thread-drill-in-keeps-anchor` in a real Chromium at 412x915 (touch) and 1280x800
# (mouse), in the dark scheme: a reply scrolled to a measured offset in All opens its thread at
# that offset, late growth above it is corrected back, Back returns it to its place, and the
# thread's root at the head of Main opens its thread where it stood. Pass WEB_ROOT=dir to run it
# against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
thread-drill-anchor-browser:
	python3 tests/thread_drill_anchor_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
