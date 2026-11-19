.PHONY: open-upstream-link-browser
# `#408 copy-upstream-link` as `#409 open-upstream-link` leaves it, in a real Chromium at 1280x800 and a
# 393x852 phone: a message carrying its Slack address offers the "Open in Slack" link second in its ⋯
# menu, wholly inside the window; the link and Shift+O open the address in a new page whose opener is
# null, this page unmoved; l copies the address whole and says so; a message with no address offers no
# item and neither key does anything. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of the open menu at each size.
open-upstream-link-browser:
	python3 tests/open_upstream_link_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
