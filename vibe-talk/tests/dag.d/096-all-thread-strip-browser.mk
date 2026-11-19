.PHONY: all-thread-strip-browser
# `#299 all-thread-strip` in a real Chromium at 1280x800 (mouse) and 412x915 (touch), in both schemes: in
# All, a strip on every row of an answered thread and on no other row, its bar at least 5px (6px under a
# finger) in its Thread(N)'s colour, its hit box 16px (22px) and the row's full height, every row's words
# at one inset, a read reply's strip still coloured, by computed colour and pixels; a click (a tap) on a
# strip: nothing scrolls or moves, still All, the rest at 0.3 and drained with grey strips; a live reply of
# the thread arriving in it and a live message of no thread dimmed; a second click, Esc, another strip and
# t; Thread(N) and Back keeping the selection; a first reply giving its root a strip without moving its
# words; a finger drawn sideways from a strip archiving nothing. Pass WEB_ROOT=dir to run it against
# another copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
all-thread-strip-browser:
	python3 tests/all_thread_strip_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
