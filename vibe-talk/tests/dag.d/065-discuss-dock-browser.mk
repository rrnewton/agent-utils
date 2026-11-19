.PHONY: discuss-dock-browser
# `#1526 discuss-docked-pane-mobile` against the real server binary: the
# half-height phone sheet docked under the list, a citation landing in the
# visible upper area, the desk overlay unchanged.
discuss-dock-browser:
	python3 tests/discuss_dock_browser.py
