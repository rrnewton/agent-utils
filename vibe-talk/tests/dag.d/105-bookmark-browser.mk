.PHONY: bookmark-browser
# `#240 reading-bookmark` in a real Chromium at 1280x800 (mouse and keys) and 412x915 (touch): the
# bookmark set on a message (Shift+B, or the row's menu) is a line in the accent drawn in the room
# between that row and the one above; scrolled away, the Bookmark chip points to it, directly right of
# Newest on the bottom line; b or the chip puts the line at the top of the list; a reply goes to the
# newest message and b comes back; and a Done on the message under the line slides it to the next, on
# the server too. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each step.
bookmark-browser:
	python3 tests/bookmark_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
