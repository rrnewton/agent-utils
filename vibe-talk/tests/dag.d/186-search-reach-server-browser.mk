.PHONY: search-reach-server-browser
# `#2269 vibe-talk-search-reach-sentence-beside-server-hits` on the shipped server (cargo build
# --release first): a search the server answers over the channel's whole history says no reach
# sentence beside its hit. BINARY=path runs another build (main before the change is the negative
# control), SCREENSHOTS=dir keeps a PNG per size.
search-reach-server-browser:
	python3 tests/search_reach_server_browser.py $(if $(BINARY),--binary $(BINARY)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
