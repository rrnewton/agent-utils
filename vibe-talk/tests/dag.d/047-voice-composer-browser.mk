.PHONY: voice-composer-browser
# `#241 voice-composer` in a real Chromium at 412x915 and 1280x800: the call's message box and the
# view switch on screen and uncovered while the box has the focus -- at idle, in a typed
# conversation, in a call, and on the phone with the on-screen keyboard's share of the screen taken
# away -- focus and typing opening nothing, and the first send opening one typed conversation. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep PNGs.
voice-composer-browser:
	python3 tests/voice_composer_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
