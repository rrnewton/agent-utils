.PHONY: thread-reader-follow-browser
# `#1856 vibe-talk-read-aloud-follow-not-yanked-up` in a real Chromium at 412x915 (touch) and
# 1280x800 (mouse), in the dark scheme: Read on a long reply in its thread follows DOWN and is
# never yanked back up across live repaints; scrolled fully off, the Reading chip rises and
# brings it back. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state.
thread-reader-follow-browser:
	python3 tests/thread_reader_follow_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
