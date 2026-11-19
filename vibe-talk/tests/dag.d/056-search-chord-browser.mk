.PHONY: search-chord-browser
# `#344 search-shortcut` in a real Chromium at 1280x800, the keys pressed through CDP: Ctrl+/ opens the
# search from the message box holding a draft (untouched), the pace popover and a ⋯ menu (closed first),
# the channel picker, Settings, Help, the right column, past a listener that stops the key, the Reply
# screen (its draft kept) and with single keys off; Ctrl+Shift+? opens Help; a German Shift+7 "/", an
# AltGr "/" and a held chord; Cmd on an emulated Mac. Pass WEB_ROOT=dir to run it against another copy
# of the page, and SCREENSHOTS=dir to keep a PNG of each step.
search-chord-browser:
	python3 tests/search_chord_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
