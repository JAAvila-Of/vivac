# Security

## Data already persisted or exposed

If sensitive information has reached a tree or escaped its intended custody,
start with [Sensitive-data incidents](docs/INCIDENT-RESPONSE.md). Revoke exposed
credentials at their issuer, stop further distribution and restrict affected
stores and copies. Closing a node, deleting an index or recovering a backup
does not sanitize its history. The guide separates actions available now from
the acceptance contract for a future sanitation operation.

## Reporting a vulnerability

A security flaw is the one report that does not go in an issue. It goes
privately:

**<https://github.com/JAAvila-Of/vivac/security/advisories/new>**

That form opens a draft advisory only you and I can read.

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

I maintain this alone, so there is no promised response time. I read every
report, answer it in the advisory, and disclose it together with its fix.
