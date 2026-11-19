.PHONY: search-unmatched-browser
# `#439 search-unmatched-separators` in a real Chromium at 393x852 (touch) and 1280x800 (mouse): a
# search's unmatched lines opened by real presses — in All, inside a thread opened under the search
# with its bar still on screen, and among the thread list's cards counting whole threads' messages,
# exactly or as at least what the page holds. Pass WEB_ROOT=dir to run it against another copy of
# the page, and SCREENSHOTS=dir to keep a PNG of each stage.
search-unmatched-browser:
	python3 tests/search_unmatched_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
