# Security

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

## What gets a fix

Only the latest release. The project is in `0.x` and
[breaks on the minor](docs/VERSIONING.md) while it is, so a fix goes into the
next release rather than back into older ones, and `vivac update` is how you
get it.

There is one maintainer, so there is no promised response time. A report is
read, answered in the advisory, and disclosed together with its fix.
