.PHONY: search-hits-pinned-browser
# `#2422 search-hits-vanish-after-redraw` in a real Chromium at 412x915 by touch and 1280x800, light
# and dark, keyboard open and closed: four result cards for an old quoted phrase
# stand through a poll's redraw. Pass WEB_ROOT=dir to run it against another copy of the page (main
# before the change leaves a blank page under "4 found (0 loaded, 4 older)"), and SCREENSHOTS=dir to
# keep a PNG per stage.
search-hits-pinned-browser:
	python3 tests/search_hits_pinned_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
