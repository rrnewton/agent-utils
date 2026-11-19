.PHONY: thread-reader-resume-browser
# `#1838 vibe-talk-reader-continues-into-thread` in a real Chromium at 412x915 (touch) and
# 1280x800 (mouse), in the dark scheme: Read on the long reply in All, tapped into its thread
# mid-chunk, resumes on that chunk at the anchor's place, goes on to the next reply, resumes
# again on Back, and stops with a word in place when switched to a view without the message.
# Pass WEB_ROOT=dir to run it against another copy of the page, and SCREENSHOTS=dir to keep a
# PNG of each state.
thread-reader-resume-browser:
	python3 tests/thread_reader_resume_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
