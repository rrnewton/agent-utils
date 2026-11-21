.PHONY: discuss-new-summary-browser
# New summary (`#2073 vibe-talk-new-summary-button`) in a real Chromium at
# 412 (phone) and 1280 (desk): the fresh card over the newer range, the
# first summary kept in the timeline, over a model stub at the network edge.
discuss-new-summary-browser:
	python3 tests/discuss_new_summary_browser.py
