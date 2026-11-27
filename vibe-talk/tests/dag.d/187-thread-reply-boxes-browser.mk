.PHONY: thread-reply-boxes-browser
# `#2385 vibe-talk-thread-open-layout-first` in a real Chromium at 412x915 by touch and 1280x800: a
# thread's replies on their way stand as boxes before their bodies, the late count reads the thread at
# once, and the rows hold through the fill. Pass WEB_ROOT=dir to run it against another copy of the
# page (main before the change fails the late-count case), and SCREENSHOTS=dir to keep PNGs per case.
thread-reply-boxes-browser:
	python3 tests/thread_reply_boxes_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
