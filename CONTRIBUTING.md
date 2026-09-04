# Contributing

**Contributions are not being accepted at the moment — neither pull requests
nor issues.** Issues are turned off in the repository settings, so the
repository and this file now agree. One report is the exception, and it has a
private channel: see [Security](#security).

Not for lack of interest, and not as a judgement on anybody's code. The model
is still moving, and every decision here is justified against the
[pillars](docs/PILLARS.md) and against a design corpus that is not public.
That makes outside review unfair in one direction: a well-written patch could
be turned down for a reason its author had no way to read. Asking for work
under those terms would waste it — and leaving a report open that nobody has
undertaken to answer is worse than saying so plainly.

The tool is published to be used, not to be built jointly. That is the whole
of it for now.

If you want to build on it, the licence is `MIT OR Apache-2.0` and a fork
needs nobody's permission. Nothing here is a claim on what you do with it.

This will change. When the model settles, this file will say so.

## Security

A security flaw is the one report this repository will take, and it goes
privately:

**<https://github.com/JAAvila-Of/vivac/security/advisories/new>**

That form opens a draft advisory only you and the maintainer can read.

The report that matters most is a way past the redaction guard — the check
that refuses to write keys, personal data or file contents into the tree. It
is the mechanism behind the one pillar that holds a veto, so a bypass is not a
defect in a feature: it is a way to put into a log the very thing the tool
promises will never be in one.

Send that one privately and nowhere else. A working bypass reads as
instructions, and posting it in the open hands them out before there is a fix
— which is also why an issue tracker would have been the wrong door for it,
open or closed.
