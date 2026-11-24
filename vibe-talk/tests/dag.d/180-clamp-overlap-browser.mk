.PHONY: clamp-overlap-browser
clamp-overlap-browser:
	python3 tests/clamp_overlap_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS)) $(if $(REPORT),--report)
