# Versioning

The project is in `0.x`, and while it is, **the minor is the position that
breaks**: `0.3.x` to `0.4.0` may change a public surface, and a patch never
does.

Every release opens with what it changes on disk and what to run — see the
[changelog](../CHANGELOG.md).

## The rule has been spent twelve times

| | |
|---|---|
| `0.3.0` | stopped reading the logs `0.1.x` and `0.2.x` wrote |
| `0.4.0` | made `find` hand back handles rather than whole nodes |
| `0.5.0` | began refusing a write that opens a fenced code block |
| `0.6.0` | made `open` hand back fronts rather than whole nodes |
| `0.7.0` | did the same to `why`, for everything but the node asked about |
| `0.8.0` | wrote an event `0.7.0` stops at |
| `0.9.0` | stopped taking a closed node off the stack when it is not the top |
| `0.10.0` | retired `vivac hooks` for `vivac setup`, and gave the hook its brief as plain text |
| `0.11.0` | made setup write Claude Code's files in the folder it is run in, and refuse to run in your home folder |
| `0.12.0` | stops a version older than itself reading a tree once that tree holds lanes |
| `0.13.0` | took planting the tree away from `vivac setup`, which now refuses where there is none |
| `0.14.0` | made a bare `vivac init` ask before planting, and refuse with no terminal and no `--yes` |

Each went out as a minor for that reason, and counting them here is cheaper
than counting them once and letting the sentence go stale.

## What keeps `1.0` away

**The format on disk is not settled.** It was going to settle by moving into
SQLite; the measurement rejected that, and [`PILLARS.md`](PILLARS.md) records
the reversal where the doctrine lives.

What is left is smaller than a migration and still open: the read cost turned
out to sit in how a node is built rather than in where its bytes are stored,
and that is not something a `1.0` should promise stability across before it is
answered. **`1.0` comes after the store settles.**

The log underneath, `.vivac/events`, is plain JSON lines and nothing stops you
reading it. What is not written down anywhere is what a line means, and that
is on purpose rather than an oversight: documenting the format as a promise is
how it would stop being able to move.

## Older releases that could bite

**`0.3.0` does not read a log written by `0.1.x` or `0.2.x`.** The tool was
written in Spanish and those releases stored the event fields under Spanish
names, which `0.2.x` read through aliases. `0.3.0` speaks one language, so it
reports those lines as unreadable rather than guessing. If you have such a
log, `0.2.1` still reads it.

**Releases before `0.3.2` could park the wrong node.** `park <id> "<reason>"`
with an id that named nothing exited 0, parked whatever the focus was instead
of what you asked for, and kept the unresolved id as the reason — dropping the
reason you wrote. The event it leaves behind is indistinguishable from a
deliberate park, so the tree never says it happened. If one of your trees was
written with an earlier release, `vivac parked` is where to look: an entry
whose reason reads like an id, or a node you do not remember parking.
`vivac focus <id>` takes it back out.

**`0.12.6` is yanked.** It shipped what `0.13.0` ships, a break, under a
patch number. If `vivac --version` says `0.12.6`, the notes you need are
under `0.13.0` in the changelog, and `cargo install vivac` takes you there.

**On Windows, run `vivac update` before installing.** A running `vivac mcp`
or `vivac web` holds the executable open, and `cargo install` fails with
*os error 5* while it does. `vivac update` sets that copy aside, so the install
goes through with every session still open, and each session keeps the old
version until it restarts. In a terminal it offers to run the install as well. A vivac that answers `vivac update` with *unknown
command* predates it: close every session and any `vivac web` before
installing over that one.
