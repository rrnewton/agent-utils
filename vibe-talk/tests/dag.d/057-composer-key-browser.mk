.PHONY: composer-key-browser
# `#403 composer-key-global` in a real Chromium at 1280x800 in two columns, the keys pressed through CDP:
# Ctrl+S goes to Main's box from a thread card and the body after it; with a thread open on the right, to
# the box of the column holding the focus or the selection, from a selected message in either column, the
# search glass and the page body; from the search field to a box, and in a box it stays; on the thread
# list, back to the view it was chosen from. The Save page is refused each time. Pass WEB_ROOT=dir to run
# it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each step.
composer-key-browser:
	python3 tests/composer_key_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
