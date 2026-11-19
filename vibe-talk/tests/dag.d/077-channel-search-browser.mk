.PHONY: channel-search-browser
# `#371 channel-search-hash` in a real Chromium at 1280x800: with one Slack service, the bar's picker
# lists `#team-builds` as `team-builds`, so typing t, e, a, m into the focused picker (keys through CDP)
# chooses it; Browse channels counts the seconds of a slow first read, then finds `#team-builds-archive`
# for `Builds` and `#eng-ops` for `#ops`. Pass WEB_ROOT=dir to run it against another copy of the page,
# and SCREENSHOTS=dir to keep a PNG of each state.
channel-search-browser:
	python3 tests/channel_search_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
