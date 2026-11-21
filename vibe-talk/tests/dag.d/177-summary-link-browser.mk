.PHONY: summary-link-browser
# `#2080 vibe-talk-summary-link-stays-in-view`: summary links stay in the current
# view, landing marked or saying why, against the real server in Chromium.
summary-link-browser:
	python3 tests/summary_link_browser.py
