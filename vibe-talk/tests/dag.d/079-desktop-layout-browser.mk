.PHONY: desktop-layout-browser
# `#372 settings-header-layout` and `#373 two-column-full-width` in a real Chromium. Settings at
# 1280x800, 1920x1080 and a 393x852 phone, under a host too long for the phone's line: the host one
# quiet line, its port whole, Expand all and Open help on its row (under it on the phone), Tab in that
# order, Expand all turning to Fold all in place. Two columns at 1920x1080 and 2560x1080 over the
# agent's long replies and the owner's answer with no threads: every row of Main but the owner's across
# its column to the gutter by the divider, the owner's kept partial, the glass over no row, the right
# column's heading and empty line across it, one column still at its reading width; and a thread open
# on the right, its rows across that column. Pass WEB_ROOT=dir to run it against another copy of the
# page, and SCREENSHOTS=dir to keep each state as a PNG.
desktop-layout-browser:
	python3 tests/desktop_layout_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
