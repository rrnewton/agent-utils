.PHONY: search-typing-stays-browser
# `#2424 search-view-unstable-while-searching` in a real Chromium at 1280x800 and 412x915 by touch,
# keyboard open and closed: typing a search never moves the row being read. Pass WEB_ROOT=dir to run it
# against another copy of the page (main before the change moves it 57 to 167 px near the newest rows),
# and TRACE=dir for one JSON trace per walk.
search-typing-stays-browser:
	python3 tests/search_typing_stays_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(TRACE),--trace $(TRACE))
