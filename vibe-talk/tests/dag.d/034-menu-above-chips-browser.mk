.PHONY: menu-above-chips-browser
# `#1228 menu-above-chips` in a real Chromium at 1280x800 and 393x852: a row's ellipsis menu over the
# floating chips row, every item the topmost element at its centre. Pass WEB_ROOT=dir to run it
# against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
menu-above-chips-browser:
	python3 tests/menu_above_chips_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
