.PHONY: read-aloud-browser
# `#7 read-aloud-latency` in a real Chromium: tap to audible against audio streamed at speaking
# pace, a switch and a stop closing the response, and only a read heard to its end archived. Its
# negative control is the same tap through `<audio src>`, which must take seconds. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
read-aloud-browser:
	python3 tests/read_aloud_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
