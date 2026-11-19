.PHONY: read-aloud-blocks-browser
# `#376 read-aloud-blocks` in a real Chromium: a long message read block by block -- lit, counted,
# jumped to by a tap, moved by Previous, Next and the media keys -- against a one-session voice at
# speaking pace, with every block boundary measured on the audio clock. Its negative control is a
# voice whose first audio is slow, which must show as a gap. Pass WEB_ROOT=dir to run it against
# another copy of the page, and SCREENSHOTS=dir to keep PNGs.
read-aloud-blocks-browser:
	python3 tests/read_aloud_blocks_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
