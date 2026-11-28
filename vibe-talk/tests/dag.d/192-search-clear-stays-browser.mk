.PHONY: search-clear-stays-browser
# `#2423 search-escape-clear-moves-found-message` in a real Chromium at 1280x800 (Escape) and 412x915 by
# touch (the X), keyboard open and closed: the found message keeps its top through the clear, and a
# search closed with nothing found puts back the row read before it. Pass WEB_ROOT=dir to run it against
# another copy of the page (main before the change moves the found message by 30 to 1100 px),
# SCREENSHOTS=dir for a PNG before and after each clear, and TRACE=dir for one JSON trace per walk.
search-clear-stays-browser:
	python3 tests/search_clear_stays_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS)) $(if $(TRACE),--trace $(TRACE))
