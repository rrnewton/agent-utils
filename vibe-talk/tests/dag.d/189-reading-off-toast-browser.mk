.PHONY: reading-off-toast-browser
# `#2375 vibe-talk-reading-off-toast-covers-sent-row` in a real Chromium at 393x852 and 412x915 by
# touch and 1280x800, light and dark: a read stopped, then a message sent; "reading mode off." is
# announced but over no row. Pass WEB_ROOT=dir to run it against another copy of the page (main
# before the change floats it over the sent row), and SCREENSHOTS=dir to keep a PNG per case.
reading-off-toast-browser:
	python3 tests/reading_off_toast_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
