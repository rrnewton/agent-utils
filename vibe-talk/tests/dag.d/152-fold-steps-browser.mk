.PHONY: fold-steps-browser
# `#301 semicolon-expand-cycle` in a real Chromium at a 1280x800 desk: ; four times and : three times step
# the long messages round Expanded all, Expanded unread, collapsed read, and Collapsed all, judged by whether
# each message's text overflows its box, with the status line read; the selected message, sampled at every
# frame after each press, never moves by more than a pixel, at the newest line too; in two columns the keys
# on the right step the right column's thread alone, its reader at its foot held, and ; in Main is Main's
# own; Ctrl+;, Cmd+; and Ctrl+: step from the channel's message box at the list's foot, where a typed ;
# steps nothing, and Ctrl+; in the right column's box steps that column alone, each chord holding the box
# it was pressed in where it is, shown whole at every frame. Pass WEB_ROOT=dir to run it against another
# copy of the page, and SCREENSHOTS=dir to keep a PNG of each step.
fold-steps-browser:
	python3 tests/fold_steps_browser.py $(if $(WEB_ROOT),--web-root $(WEB_ROOT)) $(if $(SCREENSHOTS),--screenshots $(SCREENSHOTS))
