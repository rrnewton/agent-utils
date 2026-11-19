.PHONY: threads-main-browser
# `#419 threads-view-main-messages` in a real Chromium at 1280x800 and 393x852: the Threads view's chips on
# the search glass's row, Main channel among the cards with the main channel's box last, and a reply
# under one of them moving the one box. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state.
threads-main-browser:
	python3 tests/threads_main_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
