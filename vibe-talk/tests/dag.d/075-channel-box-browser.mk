.PHONY: channel-box-browser
# `#402 who-is-who-channel-box` and `#401 hidden-channels-fold` in a real Chromium at 1280x800 and a
# 393x852 phone: Who is who in Settings' channel box, following its picker, and asking for one channel on
# All channels; the hidden channels a fold headed with their count, folded, the last thing in the Channels
# section; opening it moves nothing above it, a picker change leaves it folded, and Show again works
# inside it. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a
# PNG of each state.
channel-box-browser:
	python3 tests/channel_box_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
