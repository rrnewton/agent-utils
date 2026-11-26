.PHONY: cross-view-back-browser
# `#2273 vibe-talk-cross-view-back-lands-wrong`: Back after a link jump returns to
# the view and the row it left, against the real server in Chromium.
cross-view-back-browser:
	python3 tests/cross_view_back_browser.py --schemes light
