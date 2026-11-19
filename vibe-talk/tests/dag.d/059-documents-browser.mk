.PHONY: documents-browser
# The Documents view against the real server binary (fake chat upstream only):
# list, touch fetch, reader, and a fetch error on the row, at 393x852.
documents-browser:
	python3 tests/documents_browser.py
