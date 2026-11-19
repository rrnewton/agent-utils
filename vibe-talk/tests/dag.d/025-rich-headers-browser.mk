.PHONY: rich-headers-browser
# `#242 rich-headers` in a real Chromium: each agent's tag, title and time drawn whole in the row's
# header where it fits and as its first line where it does not, the words they restate hidden and
# none lost, at 412px and a desk (and two columns), and the Settings switch drawing every first line
# as written, kept across a reload. Pass SCREENSHOTS=dir to keep PNGs.
rich-headers-browser:
	python3 tests/rich_headers_browser.py $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
