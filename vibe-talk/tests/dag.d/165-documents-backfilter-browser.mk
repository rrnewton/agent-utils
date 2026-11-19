.PHONY: documents-backfilter-browser
# `#1908 vibe-talk-documents-back-and-filter-place` against the real server binary: Back
# restores the exact starred list; clearing the filter keeps the reader's place.
documents-backfilter-browser:
	python3 tests/documents_backfilter_browser.py
