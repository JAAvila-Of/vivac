# Using it

Every command, grouped by who runs it. The [README](../README.md) says what the
tool is and why; this says how it is driven.

Nothing here has to be memorised. `vivac` on its own prints the same list, and
that printout is the one that cannot go stale.

---

## Two audiences

The tool splits in two because its readers do. **The agent writes**, from the
command line or over [MCP](SETUP.md#mcp), and never needs an interface. **The
maintainer reads**, in a terminal or in a browser.

---

## The agent writes

Capture hangs off the seams of the work: you open a node when you start, you
close it when you finish. The provenance edge is created on its own, with
nobody having to remember to declare it.

```sh
vivac push "Fix the cache adapter" --why "the session bug needs it"
vivac push "No test for expiry" --why "no way to reproduce the bug" --blocks
vivac pop "reproduced: expires at 300s, not 3600"
vivac pop "adapter fixed"
```

The stack is the path from where this line of work starts to where you are,
and it can run through nodes that are already closed. Closing or parking one
below the focus does not move you, and the brief marks it. Only closing the
focus itself steps back to its parent, and stepping back onto something that
is already closed leaves it as it was.

### Without touching the stack

A node can be recorded without stepping into it, a decision can carry what it
rejected, and a node can be marked without its state changing:

```sh
vivac add "Retry policy is undecided" --parent 1 --why "the adapter needs it"
vivac decide "Expiry stays at 300s" --reason "the session bug was never expiry"
vivac note "the corpus run is what settled it"
vivac flag 2 review --why "measured on one file, never on the corpus"
vivac promote 2
vivac park 2 "waiting on the corpus run"
```

`decide` takes `--alternative` for what was turned down and `--supersedes` for
the decision it replaces, so a reversal reads from either end. `declare`
names, after the fact, the pillar or rule a decision was judged against;
declaring the same one again replaces its sentence, keeping the old one in
the log, which is how a badly worded `--against` is fixed without inventing
a new decision. `block` marks a
node as something its parent cannot close over, and `--off` takes it back.

A finding that asks nothing of anyone, a lesson or a measurement, is a record:
write it and close it straight away, with an outcome that starts with
`Record:`. `find` and `why` still bring it back, and `open` keeps answering
what is actually left to do. Left open, records pile up until `open` stops
saying anything. A lesson learned the hard way is a record too, titled with
its mechanism and its symptom, which are the words someone will search for.
It becomes a rule only when it draws a line any piece of work can be checked
against, and then the lessons that taught it hang under the rule.

```sh
vivac add "Cold builds take 4 min on CI" --type finding --why "measured on main"
vivac done 7 "Record: 4 min cold, 40 s warm, on the default runner"
```

A node is born under the focus, which is what makes the edge free. But the
focus is wherever work was left, perhaps by another session and about
something else, so the focus is not always where new work belongs. `--parent`
on `push` opens it under the node it continues instead: the stack is rebuilt
as that node's path, as `focus` would, and the new node opens on top. A closed
or parked node is refused, since opening work under it would quietly take back
what somebody closed or put off. Work that belongs to nothing open takes
`--root` on `push`, `add` or `decide`, which gives it no parent, and on `push`
it also leaves the stack holding only the new node. Either way nothing that
leaves the stack is closed, and the command says how to get back to it.
`promote` answers a different case: something already in the tree turns out
to be a goal of its own, and it keeps where it was born.

```sh
vivac push "Guard the start callback" --parent 12 --why "the finding it fixes"
vivac push "Ship to a second team" --root --why "the first milestone is done"
```

### What governs the project

The tree also holds the rules the project is judged against. A `pillar` is an
arbiter, and its title says so: its name and what it restricts, in the
project's own words. vivac keeps no list of kinds of pillar — what governs a
project is found by reasoning about that project, and a menu would decide it
first.

A `rule` hangs under the pillar it answers to, or under the root when no
pillar owns it, which is also where a rule about how the pillars weigh against
each other goes. A rule can carry the command that verifies it, at birth with
`--arm` or later with `vivac arm`; a rule with none is one somebody has to
judge. vivac never runs an arm: it hands it to whoever is doing the checking,
through `vivac rules` or the same read over MCP.

```sh
vivac add "Security: vetoes on the spot" --type pillar --why "the tree maps where a system is weak"
vivac add "Never store a secret" --parent 7 --type rule --arm "cargo test redact" --why "a leak cannot be taken back"
```

Rules a project already keeps in files are a different matter. vivac never
reads `CLAUDE.md`, `AGENTS.md` or any memory file, and it does not guess which
of their sentences are rules, because telling a rule from the prose around it
takes judgment. Bringing them in is the agent's job, with a person deciding —
see [Migrating](MIGRATING.md). A tree with no pillar and no rule says so when
it is asked for its rules.

---

## When a premise turns out false

The case that rots a log: an assumption is refuted, and everything built on
top of it stays on the page looking exactly as valid as it did the day
before.

```
$ vivac abandon 7 "the bottleneck was I/O, never the parser" --cascade --rescue 8

  a7  The parser is the bottleneck  -> abandoned
        and 1 descendant(s) with it

  Rescued, and still born from a7:
      f8     The token cache survives the rewrite

  Their lineage crosses an abandoned node on purpose: where they
  were born does not change because it got discarded.
```

There is a fair objection to doing any of this, and it is why most tools stop
at reporting the break instead of acting on it: **cutting a link discards
intent, and nothing left behind can say what was meant.** Once the edge is
gone the reader is guessing, and a guess written down as a fact is worse than
a gap.

The objection is right about the danger and wrong that the danger is
unavoidable, and the whole difference is where the record lives. Intent is
lost when the link **is** the record — remove it and there is nothing left to
read. Here the link is not the record. The node is, and it keeps its own
reason, its outcome and its parent.

So **a rescue does not reparent.** `f8` still hangs off the assumption that
turned out to be false, because that is where it was born, and being born
somewhere is not undone by that place being wrong. What changes is state, not
lineage — which is why "what was meant" is not lost. It is one edge up, and
still on the path:

```
$ vivac why 8

  Why we are here  ->  f8
  ------------------------------------------------------------------

  a7    The parser is the bottleneck  [abandoned]
        profiles pointed at it
        = the bottleneck was I/O, never the parser
        (1 open below)
        |
        v
  f8    The token cache survives the rewrite
        it is independent of why we started

        ^^^ you are here
```

The refuted assumption stays readable, carrying both the reason it was
believed and the reason it fell, standing between the goal and the thing that
outlived it. Nothing was dropped, so nothing has to be guessed.

---

## The maintainer reads

```sh
vivac brief         where you are, what governs this point, what NOT to touch
vivac why 11        the path from the root, narrated
vivac tree          the tree, with false closes marked
vivac open          what is waiting on you, and what has been sitting
vivac find cache    every node whose text holds all the words, best first
vivac stack         the focus stack
vivac parked        DO NOT TOUCH NOW
vivac rules         the pillars, rules and invariants that govern this project
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

### `open` answers one sentence, and the order is that sentence

What is waiting on you right now, and what has been open so long you are not
working it any more. So a front that blocks its parent comes first, because a
blocker is exactly something waiting on you; among the rest, whichever holds
up more tree; at a tie, the newest. It stops at ten, because a front is two
lines and a list you have to scroll has already broken the promise of *right
now*, and the line underneath says how many were left out and how long the
oldest of those has been open.

It used to print all of them, oldest first. On the tree this project keeps of
itself that was a hundred and nine fronts across two hundred and twenty-four
lines, with the one you touched yesterday at the bottom — which is the defect
`find` had before it was given an order, in the same product, found again
because nobody had gone to look at the neighbour.

---

## The maintainer looks

```sh
vivac web           the tree in a browser, on this machine and nowhere else
```

`vivac web` draws the tree in a browser: a server somebody starts and that
dies when they close it, bound to `127.0.0.1`, reachable through a one-time
key it prints. It opens from any directory, including one that is no project
at all — the roots come from the same registry `find --everywhere` reads, and
the working directory decides one thing only, which is where `/` lands.

It has **no functions of its own.** If a page needs something the command line
does not have, that thing gets built on the command line first, so there is no
second write path for the redaction guard to be walked around, and anything
that goes wrong on a page has a command that repeats it.

> The drawing of the tree is the one place not yet held to that, and it is a
> debt rather than a design: the page walks the tree itself instead of calling
> what `vivac tree` calls, so one shape has two implementations and nothing
> compares them. Naming it here costs less than finding it later.

Where it lands is the index: which project moved, and which has been sitting
still, without going in to ask them one at a time. Inside a project, what
moved there while you were not looking, one node's lineage, and the whole
tree. They are there because a context budget and a screen are not the same
problem. The `brief` answers *where am I* in a few hundred tokens and does it
well; it was never going to answer *what changed under me while I was not
asking*.

---

## Safe stops

A vivac is the bivouac partway up a climb: a coherent state, with the stack
frozen and the identity of the code at that moment. `push`, `pop` and `park`
leave one without anybody asking.

```sh
vivac save "before touching the adapter" --next "extract the validator"
vivac restore v14   rebuilds the stack and says what changed since
vivac vivacs        the stops, latest first
vivac why v14       that one stop, whole: its label, what you were about to
                    do, and the stack it carried
```

`restore` **never touches the working tree**. Mixing context navigation with
tree manipulation gives you a branch manager worse than git.

---

## Getting it all out

`vivac tree --json` prints the whole tree: every node with its reason, its
note, its outcome, what it refers to and what it governs. It is not the
filtered view `tree` shows a person — the JSON ignores `--all` and carries the
closed and the parked as well, because an export that quietly drops what
finished is not one.

`vivac import <tree.json>` is the way back in, and it is how the trees that
predate this binary got here: it reads a tree in that JSON shape and writes
the log a tree of that shape would have written.

The log underneath, `.vivac/events`, is plain JSON lines and nothing stops you
reading it. What is not written down anywhere is what a line means, and that
is on purpose rather than an oversight: the format is still moving, which is
what keeps `1.0` away, and documenting it as a promise is how it would stop
being able to move.

---

## Colour

A person at a terminal gets bold, dim and colour on what `init` and `setup`
print, on every read, on the help and on a refusal from the redaction guard.
Anything else, an agent, a hook, a pipe, a file or `--json`, gets the same
words as plain text, without a single escape code, and the MCP server and
the session hooks stay plain even when the environment asks for colour. `NO_COLOR` turns the
styles off; `CLICOLOR_FORCE` turns them on where there is no terminal. The
words always say what the colour does: a verb is written out, never only
coloured, and a node's colour follows its kind, which the letter of its id
already names.

At a terminal, a title too long for the window is broken at the window's own
width, and what follows lines up under the title instead of starting back at
the left edge and cutting through the drawing of the tree. `COLUMNS` sets that
width by hand. Without a terminal nothing is broken: every title stays on its
one line.

---

## Exit codes

| | |
|---|---|
| `0` | fine |
| `1` | the model refuses — a closure condition is open, a name is ambiguous |
| `2` | usage |
| `3` | the redaction guard stopped the write |
| `4` | no `.vivac/` above here |
| `5` | input/output error, a tree written by a newer vivac, or a tree another process kept locked |
