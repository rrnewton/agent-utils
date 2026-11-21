# run-agent-prompt

Run one judgement-only prompt through the first agent harness that
answers — muse, then claude, then codex — and print only its final
answer. Attempts are tool-free, individually timed out, and logged to
stderr without prompt or answer text. Exit 0 when a harness answers,
69 when every harness fails, 2 for usage or config errors.

See USER_GUIDE.md for the full operator contract.
