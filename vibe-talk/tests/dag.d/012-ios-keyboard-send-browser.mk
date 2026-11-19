.PHONY: ios-keyboard-send-browser
# `ios-keyboard-send` at an iPhone's size: every sending box's return key and input mode, a real
# Enter sending, and the Send at the simulated keyboard's top. Chromium by default, as validation
# runs it; BROWSER=webkit runs the iOS engine where it is installed (a missing WebKit skips with
# its reason, never a failure). Pass WEB_ROOT=dir against another copy of the page.
ios-keyboard-send-browser:
	python3 tests/ios_keyboard_send_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(BROWSER),--browser $(BROWSER))
