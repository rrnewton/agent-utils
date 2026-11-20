.PHONY: one-page-complete-browser
# `#1986 vibe-talk-one-page-complete-rows` in a real Chromium at 412x915 (touch,
# scale 2.6) and 1280x800 (mouse), dark: the thread row's slug, 120 replies and
# the server's 7 unread on the first page, its card's last reply and
# participants, and no reach-back in the fake API's request log.
one-page-complete-browser:
	python3 tests/one_page_complete_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
