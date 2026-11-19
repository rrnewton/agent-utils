.PHONY: fold-pins-browser
# `#1850 vibe-talk-fold-separator-shows-pinned` in a real Chromium at 412x915 and 1280x800.
fold-pins-browser:
	python3 tests/fold_pins_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
