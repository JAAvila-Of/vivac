# Setting it up

One command leaves a project ready: the hooks that hand the agent its brief,
the server it calls the tree through, and the skill it follows to bring in
what the project already knows.

```sh
vivac setup claude-code
```

Run it in the folder you open your agent in.

---

## What it writes, and where

Claude Code reads its settings and its MCP servers only from the folder it was
opened in, not from the folders above, so that is where setup writes them. The
tree is the `.vivac/` setup finds going up from there, or a new one planted in
that folder. If you open the agent in more than one folder of the same
project, run setup in each: they all share the tree above them. The plan names
every folder before anything is written.

| File | What it gets |
|---|---|
| `.claude/settings.json` | two hooks — `SessionStart` runs `vivac session start --hook`, which hands the agent the brief when a session opens and again after a compaction; `Stop` runs `vivac session end --hook`, which leaves an automatic stop |
| `.mcp.json` | the server, which runs `vivac mcp` |
| `.claude/skills/vivac-migrate/` | the skill an agent follows to bring another record into the tree — see [Migrating](MIGRATING.md) |
| `.vivac/` | planted, if the project has no tree yet |

Everything goes into the project and nowhere else.

### It asks first, and it can be taken back

Before writing, setup shows every file it will create or add to, and the exact
command each hook and the server will run, and then it asks. `--dry-run` shows
the same and writes nothing. `--yes` writes without asking, for a script, or
for an agent that has already shown you the dry run.

setup adds to a file rather than replacing it, and keeps every key it does not
own in its place. It refuses a file it cannot parse, and an entry under its
name that it did not write. A second run finds nothing to do.

**It keeps no copy of the files it changes, and that is deliberate.** A
settings file can hold credentials in its `env` block, and a copy under
another name is no longer covered by the ignore rule that keeps the original
out of the repository. Instead, it keeps the original in memory. After
writing, it reads every file back and checks that it holds what setup meant
and that nothing else in it moved. If one does not, it puts all of them back
the way they were.

`vivac setup claude-code --undo` removes exactly what setup writes and leaves
anything that is not exactly its own. The tree is never part of it.

### Why the commands are a bare `vivac`

Never a path to the executable: these files can end up in a repository, and
such a path carries the name of the account that installed it. So `vivac` has
to be on the `PATH` the harness sees.

These are plain files in your project. Commit them if everyone who works on it
uses vivac, and keep them out of version control if only you do. `.vivac/`
never goes in: the tree is this machine's, and one copy per clone would be
several trees pretending to be one. setup says so before it writes, and leaves
a `.gitignore` inside the tree that keeps it out. `vivac check` names a tree
missing that file, and gives the command that takes an already-committed
`.vivac` back out of git.

---

## Codex

```sh
vivac setup codex
```

The same pieces, in the three places Codex reads inside a project:
`.codex/config.toml` gets the server, `.codex/hooks.json` gets `SessionStart`
and `Stop` running the same two commands, and `.agents/skills/vivac-migrate/`
gets the same skill file. The tree is planted the same way, because a
project with the three files and no tree has two hooks that exit 0 in
silence for ever. Nothing goes in your own configuration directory.

It merges, and it can be taken back, the same way the Claude Code side does
and by the same rules: it adds to a file rather than replacing it, keeps
every key it does not own where it was, refuses a file it cannot read and an
entry under its name that it did not write, and a second run finds nothing
to do. `vivac setup codex --undo` removes exactly what it wrote and leaves
anything that is not exactly its own; the tree is never part of it.

The server goes into `config.toml` between two marker comments, which is how
a later run knows which lines are its own without this binary carrying a
TOML reader it needs for nothing else. A block with one marker and not the
other is left alone and named, by both directions: where it ended is a guess,
and this tool does not guess.

**Running it is yours to do, not the agent's.** Once `.codex/` and
`.agents/` exist, Codex keeps both read-only inside its own sandbox, so an
agent working in the project cannot run setup here again, merge it or take
it back. What an agent can do is create them on a project that has neither.

**Two things setup cannot do for you either**, and it says both when it
finishes. Codex reads nothing under a project's `.codex/` until you mark that
project trusted, and that lives in your own `~/.codex/config.toml`, not in
the project. And every hook is approved on its own, against its hash, with
`/hooks` inside Codex: the first time, and whenever a hook changes.

The brief reaches the agent as plain text on the opening hook's standard
output, which Codex adds to the session as context. Above roughly 2,500
tokens it saves that context to a file and shows the model a shorter preview
instead; the brief's own budget is 1,500, so that only bites if you raise it.

The flags are the same on both sides, because the ones that matter belong to
the tree and not to a harness: `--join`, `--new-tree`, `--name`,
`--lane-name`, `--dry-run`, `--yes` and `--undo` all read here exactly as
they read there. See [where it is measured](../README.md#where-this-is-measured).

`Stop` runs on every turn rather than once at the end, so the last stop does
not depend on the session closing cleanly. The stop is only saved if the tree
changed since the previous one: a stop that repeats identically is not a stop,
it is a log. Both hooks stay quiet and exit 0 where there is no `.vivac/`.

---

## Any other harness

What the hooks call is `vivac session start` and `vivac session end`, which
are commands like any other. `--hook` makes them speak to a harness instead of
a person: the brief goes out as plain text, and what kind of start it was is
read from what the harness passes in. So the pair can be run by hand to see
exactly what a hook would do.

**Any harness that can run a command when a session opens, and put its output
in the agent's context, can call the same one.** Any MCP client can run
`vivac mcp`. What setup writes for you today is Claude Code's configuration
and Codex's.

---

## MCP

The tree as tools an agent can call. `vivac setup claude-code` writes the
server into the `.mcp.json` of the folder you open the agent in, and the first
time it sees it, it may ask whether to use it: say yes. Any other MCP client
runs `vivac mcp`.

**Fourteen tools.** Five are reads: `vivac_brief`, `vivac_find`, `vivac_why`,
`vivac_open` and `vivac_rules`. Nine are writes: `vivac_push`, `vivac_pop`,
`vivac_add`, `vivac_decide`, `vivac_note`, `vivac_park`, `vivac_save`,
`vivac_arm` and `vivac_declare`. The server speaks JSON-RPC over standard
input and adds no dependency: it is the binary you already installed.

Fourteen and not more, because every tool costs context in every session the
agent ever opens, so **the list is a budget and not a catalogue.** Seven of
the writes are the seams of the work: opening something, closing it, parking
it, noting it, deciding, and the safe stop. The other two are the seams of
governance: arming a rule with the command that checks it, and declaring what
a decision was judged against. Nothing else got in.

### The same budget governs what comes back

`vivac_open` returns each front as five fields — alias, kind, state, title and
lineage — rather than the whole node, because the answer to what is unfinished
is a list of names and where they hang; `vivac_why` on an alias brings the
rest. It used to return the node, which over ten thousand nodes meant
1,993,053 bytes where 599,012 will do. **A payload nobody asked for costs the
same context as a tool nobody calls.**

`vivac_why` follows the same rule for everything but the node you asked about,
which still comes back whole. The ancestors on its path carry their bodies
clipped the way the prose clips them, and its siblings, children and blockers
come back as handles. It used to return every one of them whole: `why --json`
on a node deep in this project's own tree weighed 86,894 bytes against 3,685
for the prose, and weighs 7,139 now. Across every node of three real trees,
this one among them, the JSON went from 8.8, 6.7 and 5.7 times the prose to
1.5, 1.8 and 2.1.

### What crosses projects, and what does not

`vivac_find` takes `everywhere` and `vivac_why` takes `project`, the same two
questions the command line answers. They arrived together on purpose: a hit
from another tree carries an alias, an alias means nothing outside the tree
that issued it, and finding without being able to open would be half an
answer.

**What crosses is the project's name, never its path** — a path carries
whatever the account and its directories happen to be called, and through a
tool that lands in a model's context. No write tool takes a project: writing
into a tree you are not standing in is a larger permission than reading one,
and nobody has asked for it.

### Nothing destructive is reachable from here

`abandon` discards a node and everything below it, and through a tool that
would happen without anybody seeing a command. It stays on the command line,
where somebody is looking. So do the operations that reshape a tree rather
than record work — closing another node, blocking, flagging, restoring a safe
point. Those belong to whoever maintains the tree, and they have a terminal.

### Why the writes are here at all

The command line cannot be where an agent writes. Starting the process is
8.2 ms at the median, more than the whole 5 ms budget the performance pillar
sets for writing a node, and no process design brings that down.

Over MCP the server folds the tree once and keeps it, so a write is an append
against a tree that is already there: **0.6 ms at p99 over ten thousand
nodes**, and flat in the size of the tree, because what used to grow with it
was the fold. A read straight after a write no longer pays for a second one
either.

That correctness rests on a staleness check, not on trust: if another process
wrote to the log, the tree is folded again before the operation. Eight tests
assert that what the server holds after a write equals a fresh fold of the
log, because a fast write that quietly drifts from the record would be worse
than a slow one.

### Hooks and tools are not the same offer

A hook fires whether or not anybody wanted it; a tool is called only if the
agent decides to. So the brief still arrives through `SessionStart`, where
nothing has to choose it — `vivac_brief` is for asking again mid-session, not
for the opening.

> [!IMPORTANT]
> **On Windows, stop the server before updating.** A running `vivac mcp` holds
> the executable open, so `cargo install vivac` cannot replace it and fails
> with an access-denied error — *os error 5* — that names neither MCP nor this
> command, and so does not lead back to the cause. Close the session that
> started the server, then install. Linux and macOS replace a running binary
> without complaining.

---

## Where it stores things

The store is `.vivac/` in the project: three files — the log, the config, and
a derived index that can be deleted without changing any command's output.

There is a second place, and it is the only thing this binary puts in your
home directory: `~/.vivac/`, one per machine, holding a registry of the trees
the machine has seen. A project enters it by being used — every command
already knows the root it is standing in, so registering it is an effect of
the work rather than a step to remember, and nothing goes looking through your
disk. Entries are keyed by the id of each project's first event, so moving a
directory reads as the same project at a new path instead of a second one.
`VIVAC_HOME` points the whole thing elsewhere.

The search that finds a project walks up looking for a `.vivac/`, and this is
one, so it skips it: a directory under your home with no project above it
refuses rather than resolving to your home. What it skips is recognised by
holding the registry, not by sitting at a particular path, which is what keeps
the rule true once `VIVAC_HOME` has moved the store.

**It holds absolute paths and it stays here.** Nothing sends it anywhere, and
it lives outside every project, so no repository carries it off by accident.
Deleting it costs you the list until each tree is next used, and costs no tree
anything at all.
