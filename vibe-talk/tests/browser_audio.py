"""Keep headless audio checks independent of the host sound server.

Chromium's test switch replaces only the final OS output stream. The browser still decodes,
schedules and consumes audio on its real media clock, including media-element buffering.
See chromium/src commit f29eb01290cd36a30177ecf8197f906c01088a0d. Physical speakers are checked
separately on the device; a stalled host audio socket must not freeze these browser checks.
"""

CHROMIUM_AUDIO_ARGS = ["--disable-audio-output"]
