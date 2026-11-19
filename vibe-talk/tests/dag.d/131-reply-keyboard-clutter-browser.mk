.PHONY: reply-keyboard-clutter-browser
# `#1680 inline-reply-keyboard-clutter` in a real Chromium at 393x852 by touch, the on-screen keyboard
# simulated by a shorter viewport: replying, one Send only (the reply's own), no floating chips over the
# form, the reply's row compact on one line; in the main composer the corner Send alone; the keyboard
# down brings the chips and helper text back; at 1280x800 with a mouse none of it. Pass WEB_ROOT=dir to
# run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
reply-keyboard-clutter-browser:
	python3 tests/reply_keyboard_clutter_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
