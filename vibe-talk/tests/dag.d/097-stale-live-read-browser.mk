.PHONY: stale-live-read-browser
# `#310 stale-live-read-flicker` in a real Chromium at 1280x800: in All on its newest line, two live
# messages, the second pushed once the first one's read has reached the server, each live read answered
# 300 ms late as the server had the channel when it arrived; the second message's row drawn and laid out
# in every animation frame from its first until both reads have landed and 30 frames more. Its negative
# control runs the same walk against a copy of the page with the fix taken out, which must miss the row
# in some frame. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each state.
stale-live-read-browser:
	python3 tests/stale_live_read_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
