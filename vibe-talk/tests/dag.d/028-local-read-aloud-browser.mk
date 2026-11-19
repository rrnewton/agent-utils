.PHONY: local-read-aloud-browser
# `#561 local-tts-provider` in a real Chromium: the bar names the Local voice, Settings keeps it
# as the selected reader, and a tap is heard, at 393x852 and on a desktop. Pass WEB_ROOT=dir to
# run it against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
local-read-aloud-browser:
	python3 tests/local_read_aloud_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
