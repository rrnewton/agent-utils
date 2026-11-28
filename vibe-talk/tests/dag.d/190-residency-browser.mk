.PHONY: residency-browser
# `#2427 vibe-talk-windowed-default-on` in a real Chromium at 412x915 by touch and 1280x800: a long
# session keeps its newest window, the reading row stays put, and the step back after the cap has no
# hole. Pass WEB_ROOT=dir to run it against another copy of the page (main before the change keeps
# every row), and SCREENSHOTS=dir to keep a PNG per case.
residency-browser:
	python3 tests/residency_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
