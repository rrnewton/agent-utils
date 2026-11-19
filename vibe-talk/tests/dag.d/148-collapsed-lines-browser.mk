.PHONY: collapsed-lines-browser
collapsed-lines-browser:
	python3 tests/collapsed_lines_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS)) $(if $(REPORT),--report)
