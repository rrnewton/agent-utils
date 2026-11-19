.PHONY: done-sync-browser
# `#306 done-stale-across-views` and `#311 sent-read-waits-for-done` in a real Chromium: on a 412x915
# phone, a Done tapped in All drawn in Main, held from earlier, and an Undo in Main drawn in All, from the
# first frame each view shows and with no read; at 1280x800 in two columns, a Reply sent while the Done of
# the message it answers is held on the server asks the provider nothing until that Done is saved, and
# nothing once it is refused. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each stage.
done-sync-browser:
	python3 tests/done_sync_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
