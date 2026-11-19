.PHONY: elision-browser
# `#244 elision-word-count` in a real Chromium, in seven layouts (phones at 360 and 412px, at 80% and
# 150% text, a desk, the widest reading column, two columns): every folded row's one label claims no
# more than its clamp really hides, by the rects of its characters; Chinese and a bare address are
# counted in characters; a table the clamp draws whole says nothing, and a table over prose, a code
# block and a list of short items say no more than they hide; a rich header's row holds with the switch
# on and off. Pass WEB_ROOT=dir to run
# it against another copy of the page, and SCREENSHOTS=dir to keep a PNG of each layout.
elision-browser:
	python3 tests/elision_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
