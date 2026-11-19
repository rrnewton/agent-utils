.PHONY: read-aloud-views-browser
# `#540 read-aloud-desktop` in a real Chromium: a message read aloud from every view (Main, All, the thread
# list's cards and rows, one thread, two columns) by the agent's voice and the device's, the audio started
# on a clock this check reads and the light on the message read; a voice answering with silence stops the
# read unarchived. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep PNGs.
read-aloud-views-browser:
	python3 tests/read_aloud_views_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
