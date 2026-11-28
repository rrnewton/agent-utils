.PHONY: filter-walk-note-browser
# `#2453` in a real Chromium at 1280x800 and 412x915 by touch: the filter walk-back's landing line is
# said and floats over no row's text. Pass WEB_ROOT=dir to run it against another copy of the page
# (main before the change floats it over the landed rows) and SHOTS=dir for a PNG per walk.
filter-walk-note-browser:
	python3 tests/filter_walk_note_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SHOTS),--screenshots $(SHOTS))
