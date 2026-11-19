.PHONY: discuss-link-browser
# `#1911 vibe-talk-discuss-message-link-opens-new-window` against the real server binary: a
# Discuss citation jumps in the same window, landing visible and marked.
discuss-link-browser:
	python3 tests/discuss_link_browser.py
