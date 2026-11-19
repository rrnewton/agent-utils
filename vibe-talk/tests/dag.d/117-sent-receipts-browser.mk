.PHONY: sent-receipts-browser
# `#500 sent-receipts-pile-up` in a real Chromium at 1280x800: receipts an earlier build saved for replies
# long confirmed are not drawn on load; a reply sent from under an unpacked row of the Threads view is drawn
# under it while it goes; the Sent receipts of thread replies drawn nowhere in the view go once shown, are
# never saved, and a reload draws none while each reply is in its thread. Pass WEB_ROOT=dir to run it
# against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each state.
sent-receipts-browser:
	python3 tests/sent_receipts_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
