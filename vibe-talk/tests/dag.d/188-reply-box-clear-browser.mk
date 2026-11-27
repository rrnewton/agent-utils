.PHONY: reply-box-clear-browser
# `#2395 vibe-talk-reply-box-covers-last-card` in a real Chromium at 412x915 by touch and 1280x800:
# Reply on the last card leaves it wholly above the box. Pass WEB_ROOT=dir to run it against another
# copy of the page (main before the change fails the last-card cases), SCREENSHOTS=dir for PNGs.
reply-box-clear-browser:
	python3 tests/reply_box_clear_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
