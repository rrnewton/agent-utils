.PHONY: keyboard-browser
# `#224 keyboard-shortcuts` in a real Chromium at 1280x800, the keys pressed through CDP
# Input.dispatchKeyEvent so the browser's own defaults run: PgUp, PgDn, Space, Home and End move the
# list on screen with the focus where the page put it on load and after a view switch; "/" and Ctrl+S
# open the search (the browser's Save page refused); "," and Ctrl+, open Settings and Escape comes
# back; in the message box PgUp pages the list and Ctrl+Home/End jump it; then the keymap: j, k and the
# arrows move a visible focus ring, e marks Done and moves on, PgDn brings the ring to the page, Shift+E
# marks one opened message Done, asks the chat service once to mark it read, and folds it with nothing
# on screen moving (`#295 shift-e-read-collapse`), Enter folds, and -> enters a thread that <- and Esc
# leave; Ctrl+K and a channel chosen give the keys back to the list, and Esc leaves the picker;
# `#281 view-keys`: Ctrl+1 to Ctrl+4, and Cmd on an emulated Mac,
# choose Main, All, the thread list and two columns from the message box, the search and a card, the
# browser's default refused, and Esc on the thread list goes back to what it was chosen from. Pass
# WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each step.
keyboard-browser:
	python3 tests/keyboard_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
