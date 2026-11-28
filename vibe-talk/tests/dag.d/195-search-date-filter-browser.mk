.PHONY: search-date-filter-browser
# `#2425 vibe-talk-search-date-filter` in a real Chromium at 412x915 by touch and 1280x800, light and
# dark, keyboard open and closed: the date hint under the open bar, the calendar's panel, Last 7
# written as an absolute after:, and a typed after:7d leave only the week's hits. Pass WEB_ROOT=dir
# to run it against another copy of the page (main before the change has no calendar or hint), and
# SCREENSHOTS=dir to keep a PNG per stage.
search-date-filter-browser:
	python3 tests/search_date_filter_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
