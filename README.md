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

**The maintainer reads.**

```sh
vivac brief         where you are, what governs this point, what NOT to touch
vivac why 11        the path from the root, narrated
vivac tree          the tree, with false closes marked
vivac open          the open fronts, each with its lineage
vivac find cache    every node whose text holds all the words, newest first
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

**And the maintainer looks.** `vivac web` draws the tree in a browser, on this
machine and nowhere else: a server somebody starts and that dies when they
close it, bound to `127.0.0.1`, reachable through a one-time key it prints.

```sh
vivac web           the tree in a browser, on this machine and nowhere else
```

It has **no functions of its own.** Every page calls the same function the
command calls, so there is no second write path for the redaction guard to be
walked around, and anything that goes wrong on a page has a command that
repeats it. If a page needs something the command line does not have, that
thing gets built on the command line first.

Today it serves one page: what moved while you were not looking. It is there
because a context budget and a screen are not the same problem. The `brief`
answers *where am I* in a few hundred tokens and does it well; it was never
going to answer *what changed under me while I was not asking*.

**And there are safe stops.** A vivac is the bivouac partway up a climb: a
coherent state, with the stack frozen and the identity of the code at that
moment. `push`, `pop` and `park` leave one without anybody asking.

```sh
vivac save "before touching the adapter" --next "extract the validator"
vivac restore v14   rebuilds the stack and says what changed since
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
note or outcome holds all of the words, newest first, each with the lineage it
hangs from. Closed nodes are included on purpose: what you go looking for
months later is usually finished.

The `brief` is deterministic by contract: same log, same `--now`, same bytes.
The spine — the path from the root to the focus — is **never truncated**: if it
does not fit the budget it comes out anyway, and the warning says that what is
left over is tree, not render.

Measured on this machine, excluding process startup:

| nodes | `push` | `brief` | `tree` |
|---|---|---|---|
| 100 | ~5 ms | ~5 ms | ~5 ms |
| 1,000 | ~11 ms | ~10 ms | ~13 ms |
| 10,000 | ~54 ms | ~63 ms | ~95 ms |

The write budget is p99 < 5 ms and the read budget < 50 ms over 10,000 nodes.
In the low hundreds of nodes it holds, and from there up it degrades linearly
with the size of the log, which is read whole on every call. **That is where
SQLite comes in**, and now it has a number instead of a hunch.

Not there yet: search across projects, cascading invalidation, team mode.

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

## MCP

The reads, as tools an agent can call:

```sh
claude mcp add vivac -- vivac mcp
```

Four of them today — `vivac_brief`, `vivac_find`, `vivac_why`, `vivac_open`. It
speaks JSON-RPC over standard input and adds no dependency: the server is the
binary you already installed, and the writes keep going through the CLI.

Four because every tool costs context in every session, and that is a real
cost. It is not, however, a reason to ship a short list forever: the budget
belongs to whoever launches the server, not to whoever wrote it. The writes
belong here too, behind a flag that says how much of the surface this session
should see.

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
rather than a description of the current version. The store is `.vivac/`, two
files.

## Versioning

The project is in `0.x`, and while it is, **the minor is the position that
breaks**: `0.3.x` to `0.4.0` may change a public surface, and a patch never
does. The rule has already been spent once — `0.3.0` stopped reading the logs
`0.1.x` and `0.2.x` wrote, and went out as a minor for that reason.

**The format on disk is not settled either**, and that is what keeps `1.0`
away. Moving the store from a folded JSON log to SQLite is a change of format
already known to be coming, and a `1.0` before it would be promising stability
across a migration that is on the list. `1.0` comes after the store settles.

## Contributing

Not for now — neither pull requests nor issues. The reason is in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

## Licence

`MIT OR Apache-2.0`, at the option of whoever uses it. The text of each is in
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
