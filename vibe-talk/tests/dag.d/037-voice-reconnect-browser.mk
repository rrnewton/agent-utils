.PHONY: voice-reconnect-browser
# `#352 voice-reconnect-on-bridge-error` in a real Chromium: a fake vibe-talk-v1 bridge loses a turn
# after an interrupt and a relayed message, and sends "turn N never started after an interrupt;
# reconnect". The call stays up on a second session -- the deployed shape (a greeting, no replay) with
# the bridge closing the lost socket at once, and a bridge that lists replay, handed this call's record
# first -- the relayed message goes again once, the microphone reaches only the new session, and the
# page says so; a second lost turn within the minute ends the call as reconnect_limit, and any other
# error still ends it as error_frame. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep PNGs.
voice-reconnect-browser:
	python3 tests/voice_reconnect_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
