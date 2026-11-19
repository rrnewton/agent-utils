.PHONY: undo-chip-stretch-browser
# `#1725 vibe-talk-undo-chip-stretched` in a real Chromium at 412x915 (device scale 2.6) by touch in
# light and dark, Discuss half-pane docked open, Undo up, the keyboard simulated by a shorter
# viewport: the chip keeps a chip's height and stays clear of the reply box and Send reply, keyboard
# up and down; at 1280x800 with a mouse it never moves or stretches. Pass WEB_ROOT=dir to run it
# against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
undo-chip-stretch-browser:
	python3 tests/undo_chip_stretch_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
