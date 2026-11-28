.PHONY: reply-keyboard-head-browser
# `#2437 vibe-talk-reply-keyboard-hides-message-top` in a real Chromium at 412x915 and 393x852 by touch: the
# keyboard coming up over a Reply keeps the answered message's header on screen. Pass WEB_ROOT=dir to run it
# against another copy of the page (main before the change puts the header above the screen), and
# SCREENSHOTS=dir to keep a PNG per case.
reply-keyboard-head-browser:
	python3 tests/reply_keyboard_head_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
