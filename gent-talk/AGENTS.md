# gent-talk — agent guide

Rules specific to this utility. The repository-wide guide at the root applies
as well.

## Say who is speaking

**Prefix every message you write with an identifier saying it came from an
agent**, and which one:

    [opus 5] the assistant is reading the channel again; the tool list was stale

This applies to:

- GitHub issue and pull-request comments
- pull-request descriptions
- **commit message bodies**

It does **not** apply to commit message titles, which should stay clean.

The owner sometimes prefixes his own messages with `[human]`. The point is that
anyone reading a thread later can tell at a glance which voices are people and
which are agents, without inferring it from tone. That distinction matters most
in exactly the places it is easiest to lose — a long issue thread months later,
or a commit body quoted out of context.

Use the model you actually are, not a generic label.

## Close what you finish

If work is delivered, **close the issue**. Do not leave it open with a comment
saying it is done, waiting for someone to notice.

If part of it is delivered, say precisely what remains and leave it open for
that reason — not as a way of avoiding the decision. "Substantially complete"
on an open issue is not a status; it is a deferral.

If something is delivered but unverified in some respect, close it and say what
is unverified. A new problem found later is a new issue, not evidence that the
old one should have stayed open.
