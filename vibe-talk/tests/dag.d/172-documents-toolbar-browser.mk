.PHONY: documents-toolbar-browser
# `#2001 vibe-talk-documents-toolbar-star-downloaded` against the real server binary: the
# Downloaded/Starred pair beside the hamburger, both filters, keyboard and reload.
documents-toolbar-browser:
	python3 tests/documents_toolbar_browser.py
