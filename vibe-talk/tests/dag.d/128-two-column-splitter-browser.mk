.PHONY: two-column-splitter-browser
# `#1186 two-column-splitter` in a real Chromium at 1280x800: the separator between the two columns
# dragged resizes them with neither column's anchor row moving, stops at 300px a column,
# double-clicks back to half and half, moves by arrow keys, and survives a reload. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir for PNGs.
two-column-splitter-browser:
	python3 tests/two_column_splitter_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
