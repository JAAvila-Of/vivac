# vivac

[![ci](https://github.com/JAAvila-Of/vivac/actions/workflows/ci.yml/badge.svg)](https://github.com/JAAvila-Of/vivac/actions/workflows/ci.yml)

**A tree where every node knows which node it was born from.** It exists to
answer *"why are we here?"* months later, when nobody remembers any more.

```
$ vivac why 11

  Why we are here  ->  t11
  ------------------------------------------------------------------

  g1    vivac 0.1 publishable
        A provenance system for work that can answer "why are we
        here" months later.
        (7 open / 4 closed below)
        |
        v
  t8    Port to Rust in the public repo
        When the format stops moving, not before.
        (3 open below)
        |
        v
  t11   Redaction guard on write
        Security pillar. Goes BEFORE any cloud mode.

        ^^^ you are here

  In parallel, still open (2):
      t9     Web interface for the maintainer
      t10    Migrate from JSON to SQLite

  t8 does not close until these close (1):
      t11    Redaction guard on write
```

## The problem

When you develop with an agentic AI, work spawns more work. Three hops in, you
have lost the thread of what you originally set out to do.

It is not a memory problem: usually everything is written down. **It is a
provenance problem.** What is written does not say what it was born *from*,
and without that edge there is no way to reconstruct why you are where you
are.

Measured on a real compiler: the path between the goal and the day's work was
**six levels deep**, spread across a chronologically ordered 8,853-line
tracker, 52 planning documents and 21 issues. The structure was temporal,
which is exactly the opposite of provenance.

Logbooks, ADRs, issue trackers and session memory for agents all store the
**node**. None of them stores the **edge**. That is how you can have
everything written down and still not be able to say where something came
from.

[Where it sits](docs/POSITION.md) works through that category by category,
and says where each of them is better than this.

## How it is used

There are two audiences, and the tool splits in two because of them.

**The agent writes.** Capture hangs off the seams of the work: you open a node
when you start, you close it when you finish. The provenance edge is created
on its own, with nobody having to remember to declare it.

```sh
vivac push "Fix the cache adapter" --why "the session bug needs it"
vivac push "No test for expiry" --why "no way to reproduce the bug" --blocks
vivac pop "reproduced: expires at 300s, not 3600"
vivac pop "adapter fixed"
```

Not all of it happens on the stack. A node can be recorded without stepping
into it, a decision can carry what it rejected, and a node can be marked
without its state changing:

```sh
vivac add "Retry policy is undecided" --parent 1 --why "the adapter needs it"
vivac decide "Expiry stays at 300s" --reason "the session bug was never expiry"
vivac note "the corpus run is what settled it"
vivac flag 2 review --why "measured on one file, never on the corpus"
vivac promote 2
vivac park 2 "waiting on the corpus run"
```

`decide` takes `--alternative` for what was turned down and `--supersedes` for
the decision it replaces, so a reversal reads from either end. `block` marks a
node as something its parent cannot close over, and `--off` takes it back.

**The maintainer reads.**

```sh
vivac brief         where you are, what governs this point, what NOT to touch
vivac why 11        the path from the root, narrated
vivac tree          the tree, with false closes marked
vivac open          what is waiting on you, and what has been sitting
vivac find cache    every node whose text holds all the words, best first
vivac stack         the focus stack
vivac parked        DO NOT TOUCH NOW
vivac triage        what can be pruned, and with which command
vivac reconcile     files that changed with nothing in the tree claiming them
vivac changes       what a stretch of work opened, closed and marked
vivac stats         the numbers
vivac check         the invariants; this one belongs in CI
```

Everything the agent needs to do can be done from the command line, with no
interface in the way, and every one of those reads takes `--json` — every one
but the `brief`, which is written to be injected into a session and read as
prose, never parsed.

Some of them carry more than the line suggests. `why --full` adds the anchor,
the standing decisions and the open siblings at every step of the path, which
is the difference between a route and a briefing. `check --gates` widens the
invariants from this tree to every tree on the machine that nobody has opened,
because a tree nobody opens is where an invariant goes to break quietly. And
`open --all` drops the cap, for the times you do want the whole wall.

**`open` answers one sentence, and the order is that sentence.** What is
waiting on you right now, and what has been open so long you are not working it
any more. So a front that blocks its parent comes first, because a blocker is
exactly something waiting on you; among the rest, whichever holds up more tree;
at a tie, the newest. It stops at ten, because a front is two lines and a list
you have to scroll has already broken the promise of *right now*, and the line
underneath says how many were left out and how long the oldest of those has
been open.

It used to print all of them, oldest first. On the tree this project keeps of
itself that was a hundred and nine fronts across two hundred and twenty-four
lines, with the one you touched yesterday at the bottom — which is the defect
`find` had before it was given an order, in the same product, found again
because nobody had gone to look at the neighbour.

**And the maintainer looks.** `vivac web` draws the tree in a browser, on this
machine and nowhere else: a server somebody starts and that dies when they
close it, bound to `127.0.0.1`, reachable through a one-time key it prints.
It opens from any directory, including one that is no project at all: the
roots come from the same registry `find --everywhere` reads, and the working
directory decides one thing only, which is where `/` lands.

```sh
vivac web           the tree in a browser, on this machine and nowhere else
```

It has **no functions of its own.** If a page needs something the command line
does not have, that thing gets built on the command line first, so there is no
second write path for the redaction guard to be walked around and anything that
goes wrong on a page has a command that repeats it.

The drawing of the tree is the one place that is not yet held to that, and it
is a debt rather than a design: the page walks the tree itself instead of
calling what `vivac tree` calls, so one shape has two implementations and
nothing compares them. Naming it here costs less than finding it later.

Where it lands is the index: which project moved, and which has been sitting
still, without going in to ask them one at a time. Inside a project, what
moved there while you were not looking, one node's lineage, and the whole
tree. They are there because a context budget and a screen are not the same
problem. The `brief` answers *where am I* in a few hundred tokens and does it
well; it was never going to answer *what changed under me while I was not
asking*.

**And there are safe stops.** A vivac is the bivouac partway up a climb: a
coherent state, with the stack frozen and the identity of the code at that
moment. `push`, `pop` and `park` leave one without anybody asking.

```sh
vivac save "before touching the adapter" --next "extract the validator"
vivac restore v14   rebuilds the stack and says what changed since
vivac vivacs        the stops, latest first
```

`restore` **never touches the working tree**. Mixing context navigation with
tree manipulation gives you a branch manager worse than git.

## The two edges

It is the distinction that holds the model up, and it came out of seeding two
real trees and putting them side by side:

|                | Question it answers | When it is created |
|---|---|---|
| **born from**  | where did this come from? | on its own, at every `push` |
| **`--blocks`** | does this stop its parent from closing? | explicitly |

A closed batch of issues with an open finding underneath is **correct**: the
batch finished and the finding is another thing. An audit marked `DONE` with
its findings open is a **false marker** — one of those took 26 days to be
spotted. Same shape, opposite verdict.

That is why `vivac done` **refuses** to close with open conditions and lists
what is missing. It is the only rule in the model that rejects an operation,
and it earns that privilege because the case it prevents is measured.

```
$ vivac done 8

  t8 CANNOT close: 1 open closure condition(s)

      t11    Redaction guard on write

  A run closes with its findings, not with its report.
  Closing it anyway leaves a trace:  vivac done 8 --force
```

## When a premise turns out false

The two edges above answer where something came from and what stops it from
closing. There is a third case, and it is the one that rots a log: an
assumption is refuted, and everything built on top of it stays on the page
looking exactly as valid as it did the day before.

```
$ vivac abandon 2 "the bottleneck was I/O, never the parser" --cascade --rescue 4

  a2  The parser is the bottleneck  -> abandoned
        and 1 descendant(s) with it

  Rescued, and still born from a2:
      f4     The token cache survives the rewrite

  Their lineage crosses an abandoned node on purpose: where they
  were born does not change because it got discarded.
```

There is a fair objection to doing any of this, and it is the reason most
tools stop at reporting the break instead of acting on it: **cutting a link
discards intent, and nothing left behind can say what was meant.** Once the
edge is gone the reader is guessing, and a guess written down as a fact is
worse than a gap.

The objection is right about the danger and wrong that the danger is
unavoidable, and the whole difference is where the record lives. Intent is
lost when the link **is** the record — remove it and there is nothing left to
read. Here the link is not the record. The node is, and it keeps its own
reason, its outcome and its parent.

So **a rescue does not reparent.** `f4` still hangs off the assumption that
turned out to be false, because that is where it was born, and being born
somewhere is not undone by that place being wrong. What changes is state, not
lineage.

Which is why "what was meant" is not lost. It is one edge up, and still on the
path:

```
$ vivac why 4

  g1    Make the parser faster
        profiles pointed at it
        |
        v
  a2    The parser is the bottleneck  [abandoned]
        measured on one file, never on the corpus
        = the bottleneck was I/O, never the parser
        |
        v
  f4    The token cache survives the rewrite
        it is independent of why we started
```

The refuted assumption stays readable, carrying both the reason it was
believed and the reason it fell, standing between the goal and the thing that
outlived it. Nothing was dropped, so nothing has to be guessed.

## What it never stores

A provenance tree is a map of where a system is weak and not yet fixed. That
forces a few things, and they are not negotiable:

- **No keys and no secrets.** There is a redaction guard at write time. In
  doubt it refuses and says why; it never stores in silence.
- **No personal data.** No email, no name, no home path. The `actor` on every
  event is an opaque identifier.
- **No file contents.** Only paths, references and prose about what was
  decided. A write that opens a fenced code block is refused. It bounds the
  blast radius of a leak to *what was being worked on*, never to *what the code
  is*.
- **No telemetry.** The binary does not phone home.

These rules come from the [pillars](docs/PILLARS.md), which govern by
definition: **security vetoes, performance budgets, UX proves a surface is worth
reading, DX judges.**

## Status

**Tier 0 complete.** The tree, the two edges, the closure rule, the redaction
guard, the `brief` with its token budget, the session hooks, the vivacs and the
`Anchor` with its `Git` and `Null` implementations. The suite runs on every
pull request, on Linux, macOS and Windows; twelve of its tests are the brief
specification's contract, executed against the real binary.

`reconcile` is the first of Tier 1. It answers the one question that keeps the
tree honest -- *what changed since the tree last looked, and which of it does
no node claim?* -- by diffing the anchor's history against the `governs` globs
the nodes declare. It reports and never writes: it can say nobody claims a
file, and it cannot say which thread that file belongs to.

`find` is the other half of reading. It returns every node whose title, reason,
note or outcome holds all of the words, best first, each with the lineage it
hangs from. Closed nodes are included on purpose: what you go looking for
months later is usually finished.

**Ranking is not recency**, and the difference is the whole point. Newest-first
answers "what was I just doing"; a search answers "where was this decided", and
the nodes that decided something are the old ones. So three keys, read in
order: the field the term hit -- a title outranks a reason, a reason outranks a
note or an outcome -- then how much tree the node holds up, and only then how
recent it is. No weights, no tunable constants: the judgement is in the order
of the keys, where it can be argued with.

`find --everywhere` asks the same question of every project this machine has
seen rather than the one you are standing in. It reads the registry, so it
works from anywhere, including a directory with no tree above it at all, and
it groups the answer by project because an alias only means something inside
its own tree. It reads each project's index instead of folding its log, and it
never writes: searching from one project does not touch another's `.vivac/`.

An alias from another tree is not addressable on its own, so `why` takes
`--project`, naming a project by its directory name or by a path. A name that
matches two projects is refused rather than guessed, because answering about
the wrong tree looks exactly like answering about the right one.

The browser face came after those and answers the same way. It opens from any
directory, including one with no tree above it at all: the roots come from that
same registry, and where you are standing decides only where `/` lands -- on
the project you are inside, or on the index of all of them when you are inside
none. A project answers to its own name while that name belongs to one project
and to the id of its first event always, which is the form a saved link should
carry. A name two projects share resolves to neither and returns the page that
lets you pick, for the reason `--project` refuses to guess on the command line.

The `brief` is deterministic by contract: same log, same `--now`, same bytes.
The spine — the path from the root to the focus — is **never truncated**: if it
does not fit the budget it comes out anyway, and the warning says that what is
left over is tree, not render.

Measured at ten thousand nodes, 200 calls per cell, p50 / p99 in milliseconds,
on a tree with its derived index in place — which is what a tree has after the
first read of it. The CLI columns start a fresh process every time and include
what that costs; the MCP columns are a resident server, which is how an agent
calls.

**And it is measured twice, because a number was hiding a variable.** What
`brief`, `open` and `tree` cost is governed less by how many nodes a tree holds
than by how many of them are still open, and the shape of the tree is the one
parameter these numbers never named. Both shapes are the same ten thousand
nodes; what separates them is 170 open fronts against 3,570, and it is that
count, not a share of the tree, that these three pay for:

| | CLI, 170 open | CLI, 3570 open | MCP, 170 open | MCP, 3570 open |
|---|---|---|---|---|
| `brief` | 18.1 / 28.3 | 18.9 / 29.3 | 0.2 / 0.3 | 1.8 / 2.8 |
| `why` | 20.7 / 30.0 | 20.0 / 30.5 | 2.9 / 4.2 | 2.9 / 4.2 |
| `open` | 20.1 / 30.5 | 20.6 / 31.2 | 4.4 / 6.0 | 27.4 / 38.4 |
| `find` | 23.2 / 35.1 | 22.6 / 33.7 | 6.6 / 13.8 | 6.6 / 8.3 |
| `tree` | 21.0 / 40.1 | 25.6 / 36.3 | not a tool | not a tool |

Read `why` against `open` on the MCP columns and the variable stands on its
own: `why` costs 2.9 ms in either shape, because a lineage is bounded by depth,
while `open` goes from 4.4 to 27.4 out of the same ten thousand nodes.

**Nothing here misses the 50 ms the performance pillar gives a read, and the
table this replaces said two of them did.** Those numbers came off a fixture
whose generator exists nowhere any more, so the miss cannot be re-run,
compared, or checked — which is the charge that table was already published
under, one level down: it named the shape it was taken on and could not hand
anybody the tree. This bench is kept, and one of the things it now refuses to
do is measure a binary that finished linking moments ago, because the run that
claimed the miss was taken seconds after two compilations and every row of it
came out high, including the rows whose code had not moved.

The tree this project keeps of itself is 38% open. Whether a tree stays that
open on the way to ten thousand nodes is still not measured, and saying so
costs less than assuming it either way.

A write is p99 0.6 ms at that size over MCP, and it does not grow with the
tree: the server appends against the tree it is already holding. That figure is
from the run this table replaces, and it stands because the write path never
touches the count above.

**The CLI column used to read worse, and the tool was not.** The fixture those
numbers came from could never keep a derived index. The index is only written
when every id in the log has the shape a real one has, and the generator that
built the fixture emitted short ones, so the write declined every time and said
nothing about declining. Every call folded the whole log -- the cold path, which
a real tree takes once and then stops taking.

Side by side on one machine, one tree, one size, with nothing different but
whether the index could be kept: `tree` came back 50.7 / 62.8 without it and
22.4 / 29.4 with it. `why` came back 50.5 / 95.3 against 17.5 / 23.5.

That chase concluded the reading budget was never being missed, and it was the
right answer to a smaller question than the one worth asking. The 51.3 ms tail
it set out to explain really did come from a tree that could not cache. The
ceiling is missed anyway once half the tree is open, which nothing was looking
for, because the shape was never a number anybody wrote down. What did come out
of the chase is real and stayed: most of the cost was one write syscall per line
of output, and the crate now buffers and flushes once.

Not there yet: team mode.

**0.3.0 does not read a log written by 0.1.x or 0.2.x.** The tool was written
in Spanish and those releases stored the event fields under Spanish names,
which 0.2.x read through aliases. 0.3.0 speaks one language, so it reports
those lines as unreadable rather than guessing. If you have such a log, 0.2.1
still reads it.

**Releases before 0.3.2 could park the wrong node.** `park <id> "<reason>"`
with an id that named nothing exited 0, parked whatever the focus was instead
of what you asked for, and kept the unresolved id as the reason -- dropping the
reason you wrote. The event it leaves behind is indistinguishable from a
deliberate park, so the tree never says it happened. If one of your trees was
written with an earlier release, `vivac parked` is where to look: an entry
whose reason reads like an id, or a node you do not remember parking.
`vivac focus <id>` takes it back out and asks no permission to do it, because
parking only ever said "maybe I will be back".

## Hooks

```sh
vivac hooks     prints what to paste into .claude/settings.json
```

`SessionStart` injects the brief into the agent's context; `Stop` leaves an
automatic stop. `Stop` runs **on every turn**, not at session close — there is
no end-of-session event — so the stop is only saved if the tree changed since
the previous one: a stop that repeats identically is not a stop, it is a log.
Both stay quiet and exit 0 where there is no `.vivac/`, so they can be left in
the global configuration without getting in the way of other projects.

What they call is `vivac session start` and `vivac session end`, which are
commands like any other. `--hook` is what makes them speak the hook protocol
rather than to a person, so the same behaviour is available to anything that is
not Claude Code, and the pair can be run by hand to see what a hook would do.

## MCP

The tree as tools an agent can call:

```sh
claude mcp add vivac -- vivac mcp
```

Eleven of them: four reads — `vivac_brief`, `vivac_find`, `vivac_why`,
`vivac_open` — and seven writes — `vivac_push`, `vivac_pop`, `vivac_add`,
`vivac_decide`, `vivac_note`, `vivac_park`, `vivac_save`. It speaks JSON-RPC
over standard input and adds no dependency: the server is the binary you
already installed.

Eleven and not more, because every tool costs context in every session the
agent ever opens, so the list is a budget and not a catalogue. The seven
writes are the seams of the work — opening something, closing it, parking
it, noting it, deciding, and the safe stop — and nothing else got in.

The same budget governs what a tool hands back. `vivac_open` returns each
front as five fields — alias, kind, state, title and lineage — rather than
the whole node, because the answer to what is unfinished is a list of names
and where they hang; `vivac_why` on an alias brings the rest. It used to
return the node, which over ten thousand nodes meant 1,993,053 bytes where
599,012 will do. A payload nobody asked for costs the same context as a tool
nobody calls.

`vivac_find` takes `everywhere` and `vivac_why` takes `project`, the same two
questions the command line answers. They arrived together on purpose: a hit
from another tree carries an alias, an alias means nothing outside the tree
that issued it, and finding without being able to open would be half an
answer. **What crosses is the project's name, never its path** — a path carries
whatever the account and its directories happen to be called, and through a
tool that lands in a model's context. No write tool takes a project: writing
into a tree you are not standing in is a larger permission than reading one,
and nobody has asked for it.

**Nothing destructive is reachable from here, and that is deliberate.**
`abandon` discards a node and everything below it, and through a tool that
would happen without anybody seeing a command. It stays on the command line,
where somebody is looking. So do the operations that reshape a tree rather
than record work — closing another node, blocking, flagging, restoring a
safe point. Those belong to whoever maintains the tree, and they have a
terminal.

The writes are here because the command line cannot be where an agent writes.
Starting the process is 8.2 ms at the median, more than the whole 5 ms budget
the performance pillar sets for writing a node, and no process design brings
that down.

Over MCP the server folds the tree once and keeps it, so a write is an
append against a tree that is already there: **0.6 ms at p99 over ten
thousand nodes**, and flat in the size of the tree, because what used to grow
with it was the fold. A read straight after a write no longer pays for a
second one either.

That correctness rests on a staleness check, not on trust: if another process
wrote to the log, the tree is folded again before the operation. Eight tests
assert that what the server holds after a write equals a fresh fold of the
log, because a fast write that quietly drifts from the record would be worse
than a slow one.

Hooks and MCP are not the same offer, and the difference matters. A hook fires
whether or not anybody wanted it; a tool is called only if the agent decides to.
So the brief still arrives through `SessionStart`, where nothing has to choose
it — `vivac_brief` is for asking again mid-session, not for the opening.

**On Windows, stop the server before updating.** A running `vivac mcp` holds
the executable open, so `cargo install vivac` cannot replace it and fails with
an access-denied error — *os error 5* — that names neither MCP nor this
command, and so does not lead back to the cause. Close the session that
started the server, then install. Linux and macOS replace a running binary
without complaining, so this one is Windows only.

## Install

```sh
cargo install vivac
vivac init
```

From source, `cargo install --path .` inside the repo.

No background process, and no network in the write path — `push` is the binary
writing to a file. **The binary never phones home**, and that one is a promise
rather than a description of the current version. The store is `.vivac/`,
three files: the log, the config, and a derived index that can be deleted
without changing any command's output.

There is a second place, and it is the only thing this binary puts in your
home directory: `~/.vivac/`, one per machine, holding a registry of the trees
the machine has seen. A project enters it by being used — every command
already knows the root it is standing in, so registering it is an effect of
the work rather than a step to remember, and nothing goes looking through your
disk. Entries are keyed by the id of each project's first event, so moving a
directory reads as the same project at a new path instead of a second one.
`VIVAC_HOME` points the whole thing elsewhere.

The search that finds a project walks up looking for a `.vivac/`, and this
is one, so it skips it: a directory under your home with no project above it
refuses rather than resolving to your home. What it skips is recognised by
holding the registry, not by sitting at a particular path, which is what
keeps the rule true once `VIVAC_HOME` has moved the store.

It holds absolute paths and it stays here. Nothing sends it anywhere, and it
lives outside every project, so no repository carries it off by accident.
Deleting it costs you the list until each tree is next used, and costs no tree
anything at all.

## Getting it all out

`vivac tree --json` prints the whole tree: every node with its reason, its
note, its outcome, what it refers to and what it governs. It is not the
filtered view `tree` shows a person — the JSON ignores `--all` and carries the
closed and the parked as well, because an export that quietly drops what
finished is not one.

`vivac import <tree.json>` is the way back in, and it is how the trees that
predate this binary got here: it reads a tree in that JSON shape and writes the
log a tree of that shape would have written.

The log underneath, `.vivac/events`, is plain JSON lines and nothing stops you
reading it. What is not written down anywhere is what a line means, and that is
on purpose rather than an oversight: the format is still moving, which is what
keeps `1.0` away, and documenting it as a promise is how it would stop being
able to move.

## Versioning

The project is in `0.x`, and while it is, **the minor is the position that
breaks**: `0.3.x` to `0.4.0` may change a public surface, and a patch never
does. The rule has been spent four times — `0.3.0` stopped reading the logs `0.1.x`
and `0.2.x` wrote, `0.4.0` made `find` hand back handles rather than whole
nodes, `0.5.0` began refusing a write that opens a fenced code block, and
`0.6.0` made `open` hand back fronts rather than whole nodes. Each went out as
a minor for that reason, and counting them here is cheaper than counting them
once and letting the sentence go stale.

**The format on disk is not settled either**, and that is what keeps `1.0`
away. It was going to settle by moving into SQLite; the measurement rejected
that, and [`docs/PILLARS.md`](docs/PILLARS.md) records the reversal where the
doctrine lives. What is left is smaller than a migration and still open: the
read cost turned out to sit in how a node is built rather than in where its
bytes are stored, and that is not something a `1.0` should promise stability
across before it is answered. `1.0` comes after the store settles.

## Contributing

Not for now — neither pull requests nor issues. The reason is in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## Licence

`MIT OR Apache-2.0`, at the option of whoever uses it. The text of each is in
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
