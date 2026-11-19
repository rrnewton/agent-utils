.PHONY: scroll-soak-browser
# `#1210 ui-stability-no-async-shift` in a real Chromium at 393x852 by touch and 1280x800: a reader parked
# mid-Main with a draft held holds still for 60 s of background updates (live messages and replies, edits
# above and below, reactions, read state, summaries, the Newest recount and the poll), the anchor row
# within a pixel at every frame and approximately zero layout shift, each update named separately; a
# closing control with anchoring off proves the watch sees a shift. Pass WEB_ROOT=dir to run it against
# another copy of the page, and SCREENSHOTS=dir to keep a PNG of each stage.
scroll-soak-browser:
	python3 tests/scroll_soak_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
