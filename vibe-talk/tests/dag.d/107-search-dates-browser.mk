.PHONY: search-dates-browser
# `#249 search-highlight`, `#250 date-separators` and `#251 author-filter` in a real Chromium at
# 1280x800 (mouse) and 412x915 (touch), over a week of a channel read at a fixed noon: a rule above the
# first message of each day, the bookmark's line nearer the message than its day's; a search's hits lit,
# its gaps said with the days they reach, each line as wide as the list and touching no row; and the
# author filter showing only the owner's prompts with the one chosen left where it stood. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each stage.
search-dates-browser:
	python3 tests/search_dates_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
