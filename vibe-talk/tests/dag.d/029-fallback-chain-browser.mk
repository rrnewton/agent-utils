.PHONY: fallback-chain-browser
# `#563 read-aloud-fallback-chain` in a real Chromium: Settings displays the configured voice
# order, a tap is heard, and the page names the fallback voice that spoke the block, at 393x852
# and on a desktop. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep PNGs.
fallback-chain-browser:
	python3 tests/fallback_chain_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
