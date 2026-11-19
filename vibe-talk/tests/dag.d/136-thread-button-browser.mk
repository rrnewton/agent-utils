.PHONY: thread-button-browser
# `#252 thread-button-state` in a real Chromium at 412x915 (touch) and 1280x800 (mouse), in the dark
# and light schemes: a new message's Thread tag, whose thread nobody has answered, is drawn as Reply
# is and an answered root's Thread(N) in its thread's colour; the grey tag opens the empty thread
# with its box focused, the first reply is posted into the message's thread, and the tag turns
# coloured; at the desk, the same on the right in two columns. Pass WEB_ROOT=dir to run it against
# another copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
thread-button-browser:
	python3 tests/thread_button_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
