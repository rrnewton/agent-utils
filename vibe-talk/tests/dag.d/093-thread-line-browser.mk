.PHONY: thread-line-browser
# `#385 thread-line-overlap` in a real Chromium at 1000x800, 1280x800 (both schemes) and 1920x1080 with a
# mouse, and 393x852 by touch: the line from a thread's root to its card on the right runs down a lane at
# least 4px left of every row of the thread, whole and clipped at Main's top, and the owner's rows stay
# partial width; on the phone no line is drawn. Pass WEB_ROOT=dir to run it against another copy of the
# page, and SCREENSHOTS=dir to keep a PNG of each state.
thread-line-browser:
	python3 tests/thread_line_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
