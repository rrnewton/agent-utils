.PHONY: link-url-chip-browser
# `#620 link-url-chip` in a real Chromium at 393x852 and 412x915 (touch) and 1280x800 (mouse): the URL
# chip under Links writes each link as its whole address, wrapped at its slashes, and back as its label,
# kept over a reload and under a search by an address; and the list starts below the chip row, so no row
# shows through between its pills. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each stage.
link-url-chip-browser:
	python3 tests/link_url_chip_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
