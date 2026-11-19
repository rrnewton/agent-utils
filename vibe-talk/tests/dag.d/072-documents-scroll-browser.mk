.PHONY: documents-scroll-browser
# `#1264 documents-scroll-landings` against the real server binary: slow-fetch
# first open at the top, Back at the bottom or exactly where the list was left.
documents-scroll-browser:
	python3 tests/documents_scroll_browser.py
