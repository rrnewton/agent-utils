.PHONY: threads-polish-browser
# `#421 chip-states`, `#422 composer-key-stay`, `#424 threads-main-own-center` and `#423
# back-key-consistency` in a real Chromium at 1280x800 and 393x852: the Threads view's chips pressed alike,
# Cmd+S keeping the Threads view with Main channel on, the owner's main rows centred, and Cmd+[ and Cmd+]
# back and forward through the views, a thread and Settings. Pass WEB_ROOT=dir to run it against another
# copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
threads-polish-browser:
	python3 tests/threads_polish_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
