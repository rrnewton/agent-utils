.PHONY: wit-read-aloud-browser
# `#552 wit-tts-provider` in a real Chromium: Settings offers the Wit.ai voice and its catalogue
# picker, the picked voice is what speech preparation asks for, and a tap is heard, at 393x852
# and on a desktop. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep PNGs.
wit-read-aloud-browser:
	python3 tests/wit_read_aloud_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
