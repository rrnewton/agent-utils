.PHONY: live-relay-check-self-test
# `#354 live-relay-check`: the verdict logic of the opt-in live check of relays during a call, on
# synthetic frame logs. No browser, server or bridge.
live-relay-check-self-test:
	python3 scripts/live-relay-check.py --self-test
