.PHONY: label-chip-tap-browser
# `#2285 vibe-talk-labels-browser-tapped-message-jumps`: a tapped label chip keeps the
# tapped row's top, per frame, in Chromium. TRACES=dir keeps each scene's frames as JSON.
label-chip-tap-browser:
	python3 tests/label_chip_tap_browser.py $(if $(TRACES),--traces $(TRACES)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
