.PHONY: voice-read-acceptance-self-test
# The live #47 read-aloud-overlong check is opt-in and may consume provider time. Its transcript
# reconciliation, PCM-duration metric, re-answer separation, privacy boundaries, threshold edges,
# interruption rejection, and WAV writer are deterministic offline controls, so keep those in the
# ordinary gate.
voice-read-acceptance-self-test:
	python3 scripts/voice-read-acceptance.py --self-test
