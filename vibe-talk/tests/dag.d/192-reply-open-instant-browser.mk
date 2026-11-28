.PHONY: reply-open-instant-browser
# `#2428 vibe-talk-reply-open-local-instant` in a real Chromium at 1280x800 and 412x915 by touch: Reply shows
# its box at the first frame after the tap, no request and no whole-list redraw before it, over 100k px of
# rows. Pass WEB_ROOT=dir to run it against another copy of the page (main before the change redraws every
# row inside the tap).
reply-open-instant-browser:
	python3 tests/reply_open_instant_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
