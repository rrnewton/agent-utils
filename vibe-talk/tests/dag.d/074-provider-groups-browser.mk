.PHONY: provider-groups-browser
# `#355 provider-grouped-channels` in a real Chromium at 1280x800 and a 375x812 phone: with two chat
# services, the bar's channel picker holds one <optgroup> per service in configuration order; Alt+Down
# and Alt+Up, pressed through CDP, step across the groups in the order shown and stop at the ends; a
# reload restores the channel inside its group; Ctrl+K and a choice read that channel; on the phone the
# picker stays inside the window. Pass WEB_ROOT=dir to run it against another copy of the page, and
# SCREENSHOTS=dir to keep a PNG of each state.
provider-groups-browser:
	python3 tests/provider_groups_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
