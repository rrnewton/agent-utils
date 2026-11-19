.PHONY: discord-split-merge-browser
discord-split-merge-browser:
	python3 tests/discord_split_merge_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS)) $(if $(REPORT),--report)
