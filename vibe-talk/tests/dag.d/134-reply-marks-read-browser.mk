.PHONY: reply-marks-read-browser
# `#300 reply-marks-read` in a real Chromium at 1280x800, in two columns: a Reply docked in either column
# and the first reply typed in a new thread's box mark what they answer read — Done on the server, the
# provider asked `after_reply` after it, the row drawn read, in both columns for the thread's root — and
# a later message in an answered thread's box marks nothing. Pass WEB_ROOT=dir to run it against another
# copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
reply-marks-read-browser:
	python3 tests/reply_marks_read_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
