.PHONY: read-modes-browser
# `#221 read-modes` in a real Chromium at 412x915 and 1280x800: entering a thread lands on its first
# unread message, unfolded, under the pill, in each read mode, and Collapse read's "… N read
# messages …" line is a 44px target a real tap opens without moving the reader; then, `#222
# unread-replies`, Main under Hide read keeps the read question for its unread replies, says "4 unread"
# beside "52 replies", and a real tap on the count lands on the first of them. Pass WEB_ROOT=dir to
# run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each step.
read-modes-browser:
	python3 tests/read_modes_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
