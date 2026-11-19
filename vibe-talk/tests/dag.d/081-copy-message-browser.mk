.PHONY: copy-message-browser
# `#296 copy-highlighted-message` in a real Chromium, the keys pressed through CDP and the clipboard read
# back with `clipboard-read` granted: at 1280x800, Ctrl+C with nothing highlighted copies nothing; on the
# folded long question it copies all of it as written, with a text/html flavour, and moves nothing; in the
# message box, and with words selected, the browser's copy as ever; y copies the message, and a thread
# list card copies its first message; in a tab emulating a Mac, Cmd+C with the Edit menu's "copy" command
# and without it; on a 412x915 phone with a hardware keyboard, a tap's silent selection copies nothing and
# j's highlight does. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each state.
copy-message-browser:
	python3 tests/copy_message_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
