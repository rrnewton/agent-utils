.PHONY: thread-layout-server-browser
# `#2385 vibe-talk-thread-open-layout-first` S-t1 on the shipped server (cargo build --release
# first): the page's thread layout read reaches the real route, prices each box, and the bodies fill
# the boxes in place. BINARY=path runs another server build, WEB_ROOT=dir serves another page build
# over it (main's page before S-t1 is the negative control), SCREENSHOTS=dir keeps a PNG per stage.
thread-layout-server-browser:
	python3 tests/thread_layout_server_browser.py $(if $(BINARY),--binary $(BINARY)) $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
