.PHONY: compact-rows-browser
compact-rows-browser:
	python3 tests/compact_rows_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS)) $(if $(REPORT),--report)
