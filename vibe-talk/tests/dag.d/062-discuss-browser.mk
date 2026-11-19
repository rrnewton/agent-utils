.PHONY: discuss-browser
# Discuss (`catchup-side-agent`, slice 1) in a real Chromium at 393x852 (touch) and
# 1280x800: the panel opens, reads, marks exactly the markable ids and jumps, over a
# contract-shaped catchup answer at the network edge. Run once the server's catchup
# route and the page have landed together.
discuss-browser:
	python3 tests/discuss_browser.py
