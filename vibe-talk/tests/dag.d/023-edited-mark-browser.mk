.PHONY: edited-mark-browser
# `#320 slack-edited-mark` in a real Chromium: the "(edited)" after an edited row's time, on one line
# inside the meta line, after the time, clear of the row's buttons, in the time's ink and size, at two
# phone widths and a desk, in both schemes. Pass SCREENSHOTS=dir to keep one PNG per size and scheme.
edited-mark-browser:
	python3 tests/edited_mark_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
