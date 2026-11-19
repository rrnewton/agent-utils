.PHONY: post-confirm-browser
# `#49 post-confirmation-card-hold` in a real Chromium: a real touch on Send the moment a first
# post-confirmation card appears commits nothing, and Send enables itself after the hold. Pass
# WEB_ROOT=dir to run it against another copy of the page.
post-confirm-browser:
	python3 tests/post_confirm_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT))
