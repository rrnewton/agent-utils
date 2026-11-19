.PHONY: documents-scrubber-browser
# The multi-level document scrubber (`#1508`) against the real server binary:
# numbers on the document, navigator rows, tap-a-unit reading, both widths.
documents-scrubber-browser:
	python3 tests/documents_scrubber_browser.py
