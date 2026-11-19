.PHONY: filter-place-browser
# `#243 filter-keeps-selection` in a real Chromium at 1280x800 and 412x915, the owner's own sequence:
# search, Pinned, a pin chosen with the mouse or a finger, the filter cleared -- the pin's row stands in
# the channel's list where it stood in the Pinned list, within a pixel, and stays there; then an old pin,
# two pages beyond what is loaded, walked back to under a "Loading earlier messages…" line and put where
# it stood. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a
# PNG of each stage.
filter-place-browser:
	python3 tests/filter_place_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
