.PHONY: empty-messages-browser
# `#386 hide-empty-messages` in a real Chromium at 1280x800 (mouse) and 393x852 (touch), in both schemes:
# with the Settings switch on, its default, no empty message is drawn or counted, in All, Main or a thread
# on the right, while a file alone and a reaction alone are drawn; off, every empty message is back, kept
# across a reload. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each state.
empty-messages-browser:
	python3 tests/empty_messages_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
