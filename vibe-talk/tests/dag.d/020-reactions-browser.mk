.PHONY: reactions-browser
# `#219 emoji-reactions` in a real Chromium: every reaction strip laid out under its row's text,
# clear of the row's controls and legible, at 412px, 360px and a desk, dark and light. Pass
# SCREENSHOTS=dir to keep PNGs.
reactions-browser:
	python3 tests/reactions_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
