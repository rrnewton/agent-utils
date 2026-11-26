.PHONY: has-restyle-browser
# `#2283 vibe-talk-root-has-rules` in a real Chromium at 412x915 by touch and 1280x800, with 260
# rows in Main: one hidden toggle of each control the page toggles while typing and scrolling
# restyles a handful of elements (counted from the trace's style recalcs, never timed), and the
# page's own typing and scroll passes never restyle the whole document. Pass WEB_ROOT=dir to
# run it against another copy of the page.
has-restyle-browser:
	python3 tests/has_restyle_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
