.PHONY: discuss-split-browser
# `#2263 vibe-talk-discuss-split-pane` against the real server binary: the
# summary holds still while the timeline scrolls, the boundary drags and is
# kept, the timeline collapses with its new-summary hint, and the phone
# keyboard leaves the composer and the Latest pair in view.
discuss-split-browser:
	python3 tests/discuss_split_browser.py
