.PHONY: search-empty-column-browser
# `#2268 vibe-talk-search-none-match-line-overflow` in a real Chromium at 1280x800, 1920x1080 and
# 412x915 by touch: a search nothing loaded matches keeps its None-match line and the Search All chip
# inside the message column. Pass WEB_ROOT=dir to run it against another copy of the page (main before
# the change fails at both desk sizes), and SCREENSHOTS=dir to keep a PNG of each size.
search-empty-column-browser:
	python3 tests/search_empty_column_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
