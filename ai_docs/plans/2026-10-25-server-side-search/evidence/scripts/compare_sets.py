#!/usr/bin/env python3
"""Compare an upstream search's hits with the page's matching rule over a local copy of a channel.

Inputs, each one value a line:
    SLACK   message_ts values the upstream search returned (search_pages.py with DUMP=...)
    OURS    created_ms values of the messages whose search text contains the term as a
            lower-case substring: the page's rule (the pilot ran
            `SELECT created_ms FROM message WHERE chan = ? AND instr(search_text, ?) > 0`
            over a private copy of the server's message cache)

Messages are matched on creation time in milliseconds (floor of ts * 1000).

Usage: compare_sets.py TERM SLACK OURS
"""
import sys
from decimal import Decimal

term, slack_path, ours_path = sys.argv[1:4]
# Decimal, not float: float(ts) * 1000 can land a hair under the integer and floor 1 ms low.
slack = {int(Decimal(x) * 1000) for x in open(slack_path).read().split()}
ours = {int(x) for x in open(ours_path).read().split()}
print(f"{term:12} upstream={len(slack):3} ours={len(ours):3} "
      f"ours-not-upstream={len(ours - slack):3} upstream-not-ours={len(slack - ours):3}")
