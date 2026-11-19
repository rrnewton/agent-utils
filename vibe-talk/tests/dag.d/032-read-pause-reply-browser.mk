.PHONY: read-pause-reply-browser
# `#554 read-pause-reply-resume` in a real Chromium on a phone's screen: a read paused, the message replied
# to and sent, then resumed, by both voices (the device's modelled on Chrome for Android, whose pause stops
# and whose resume does nothing); the read goes on from where it was held. Pass WEB_ROOT=dir to run it
# against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
read-pause-reply-browser:
	python3 tests/read_pause_reply_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
