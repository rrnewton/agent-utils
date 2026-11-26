.PHONY: geometry-restyle-browser
# `#2277 vibe-talk-key-scroll-freezes-11-26` in a real Chromium at 412x915 by touch and 1280x800, with 260
# rows in Main: the page's measured geometry restyles only its readers (counted from the trace's style
# recalcs, never timed), its own scroll passes never restyle the whole document, and typing at the foot
# with the keyboard up while messages arrive never writes the list back and forth. Pass WEB_ROOT=dir to
# run it against another copy of the page.
geometry-restyle-browser:
	python3 tests/geometry_restyle_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
