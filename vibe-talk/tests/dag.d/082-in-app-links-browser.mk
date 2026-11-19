.PHONY: in-app-links-browser
# `#543 in-app-message-links` in a real Chromium at 1280x800 and a 393x852 phone with touch: a link to
# a message in a channel the app reads opens it in the app as one history entry, and a real Back returns
# to the reader's message at the same height, stacked jumps undone one at a time. Pass WEB_ROOT=dir to
# run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each landing.
in-app-links-browser:
	python3 tests/in_app_links_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
