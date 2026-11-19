.PHONY: discuss-uncovered-browser
# `#1843 vibe-talk-discuss-uncovered-messages-line` against the real server
# binary: seed non-owner messages through the fake-seed route, summarize
# through the real catch-up routes with only the model stubbed, leave a
# gap, and clean it up from the uncovered line.
discuss-uncovered-browser:
	python3 tests/discuss_uncovered_browser.py
