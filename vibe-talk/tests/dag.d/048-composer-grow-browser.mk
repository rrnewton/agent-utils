.PHONY: composer-grow-browser
# `#340 composer-grow-box` in a real Chromium: the Grow checkbox left of Send, and with it ticked a real
# long paste growing the message box to 60% of the window with Send on screen, at 1280x800, 1920x1080,
# 1280x600 and in two columns, with field-sizing turned off too, a reader at the foot kept there while
# typing and when Grow is ticked after a paste, one a little above it keeping the checkbox under the
# pointer, also by a click that focuses no checkbox as in Safari and Firefox on a Mac, one up the list
# not moved, the call's box with its recap line shown above it, and a 393x852 phone unchanged. Pass
# WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state, Grow off and on.
composer-grow-browser:
	python3 tests/composer_grow_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
