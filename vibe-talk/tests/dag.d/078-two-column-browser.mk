.PHONY: two-column-browser
# `#229 desktop-two-column` in a real Chromium at 1440x900 and 1280x800: the layout button draws Main
# and the right column side by side inside the window, the view picker saying so; `#281 view-keys`: the
# thread list chosen from the picker is that column across the window, and real Ctrl+2 and Ctrl+4
# leave and come back to two columns; the right column's cards are one height, newest activity first,
# the long one cut to three lines; a card's replies button and a Main message's Thread(N) select a
# thread there under its name, each column scrolls alone under the wheel, the X goes back to the cards,
# and a window narrowed to 900px is one column in its old view, two again when wide. Pass WEB_ROOT=dir
# to run it against another copy of the page, and SCREENSHOTS=dir to keep the list and thread states as
# PNGs.
two-column-browser:
	python3 tests/two_column_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
