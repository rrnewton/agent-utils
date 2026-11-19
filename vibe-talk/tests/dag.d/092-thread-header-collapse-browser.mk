.PHONY: thread-header-collapse-browser
# `#388 thread-header-collapse` in a real Chromium at 393x852 by touch and 1280x800 with a mouse, in both
# schemes: a tap or click on the card at the top of a thread folds it to a 14px bar of its own fill and
# edge, by computed colour and pixels, with a 24px press target, and a tap on the bar opens it; no row on
# screen moves either way, part-way up the thread or at its foot; a link in the card and its X do not fold
# it; a reload keeps it folded; Enter and Space on its button; in two columns the line from the root meets
# the bar's left edge, its lane still left of every row. Pass WEB_ROOT=dir to run it against another copy
# of the page, and SCREENSHOTS=dir to keep a PNG of each state.
thread-header-collapse-browser:
	python3 tests/thread_header_collapse_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
