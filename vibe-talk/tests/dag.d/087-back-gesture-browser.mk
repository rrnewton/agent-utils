.PHONY: back-gesture-browser
# `#304 back-gesture-in-app` in a real Chromium at a 412x915 Android phone in standalone mode: the browser's
# Back -- what the phone's back gesture sends -- closes a thread in the same document with the reader's place
# kept, the thread list and a thread opened from it are two entries closed in turn by history.back() and the
# browser's Back, the ⋯ menu over a thread is an entry the browser's Back closes alone, the on-screen Back goes
# back through its entry so the browser's Back at the top leaves the app, and a page reloaded in a thread opened
# from the list stands on the earlier load's entries, so the first Back, before any touch, closes the thread
# alone. With the app as the tab's first entry, as in an installed app, no entry is left under it after a
# reload; at a 1440x900 desk in two columns the browser's Back with a ⋯ menu open closes the menu, not the
# thread on the right. Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to
# keep a PNG of each state.
back-gesture-browser:
	python3 tests/back_gesture_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
