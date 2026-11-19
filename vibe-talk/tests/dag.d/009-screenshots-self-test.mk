.PHONY: screenshots-self-test
# The screenshot harness's offline controls: its capture judge and PNG decoder, and its scene table.
screenshots-self-test:
	python3 scripts/screenshots.py --self-test
