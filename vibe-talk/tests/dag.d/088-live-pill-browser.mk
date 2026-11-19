.PHONY: live-pill-browser
# `#324 live-pill-covers-row` in a real Chromium at a 412x915 phone with touch, in both schemes, the pill saying
# "Live · updated": in Main the unread chip scrolled onto the pill's line is clipped by the list's top edge, not
# covered, and in Main and in a thread scrolled through nine places nothing of the list is under the pill or the
# search glass, by a tap through the pill and by the pixels there; a thread entered from its root lands with the
# root's date rule below the floating line. Its controls put the old room, padding inside the scroller, back and
# find the list under the pill. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir
# to keep a PNG of each state.
live-pill-browser:
	python3 tests/live_pill_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
