.PHONY: search-clear-note-browser
# `#2442` R5 in a real Chromium at 1280x800, 1920x1080 with two columns and 412x915 by touch: the
# place-keeper's substitute sentence floats over no row, and a clear that shows the selection again
# takes it away. Pass WEB_ROOT=dir to run it against another copy of the page (main before the change
# leaves the sentence floating over a row after every clear) and SHOTS=dir for a PNG per walk.
search-clear-note-browser:
	python3 tests/search_clear_note_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SHOTS),--screenshots $(SHOTS))
