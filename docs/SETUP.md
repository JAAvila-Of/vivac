# Setting it up

Two commands, because a project ready to work in answers two questions:
which tree this folder belongs to, and what your agent reads inside it.

```sh
vivac init
vivac setup claude-code
```

Run both in the folder you open your agent in. `init` plants the tree, or
joins the one this project already has, and declares this folder's own
thread in it. `setup` writes the hooks that hand the agent its brief, the
server it calls the tree through, and the skill it follows to bring in what
the project already knows.

**It used to be one command, and the second one is the price of a real
fix.** The tree was setup's to plant, so which harness you happened to name
decided things that have nothing to do with a harness: where the tree
lives, which folder becomes a lane, what the product is called. And `vivac
init` planted as well — a different tree, with no lane declared and no
version lock — so whether the guard that stops you keeping two maps of one
product ever fired depended on which of the two commands you had used, and
nothing told you. One road to plant is what removes that, and one road is
what costs the second command.

`setup` refuses now rather than planting half a project. It names the
`init` to run and writes nothing until you have run it.

## Checking before planting

Before creating a tree, `init` checks for nearby nested trees and projects in
the local registry. It also follows plain local filesystem paths declared in
`remote.origin.url` by repositories with an ordinary `.git` directory found
in this folder. It checks each source and at most two parent directories,
stopping at the first tree or lane marker. Readable trees' last repository
declarations supply the root commit comparison. It does not enumerate siblings, rebuild
the registry, follow chains of origins or execute Git inside a source tree.
Git parses captured configuration with includes disabled; remote URLs, remote
helpers, file URLs, UNC paths and device paths are not followed.
Links in an origin path are not followed and do not establish ownership.

A matching tree stops planting even with `--yes` or `--dry-run`. Use
`vivac init --join <path to the existing tree>` to retain its history. If that
tree should live here, move it with `vivac relocate` first. A deliberate fork
can use `vivac init --new-tree`; that choice does not override the refusal for
a tree nested inside this folder. Existing linked worktrees keep their normal
resolution through their main checkout; this check does not read that
checkout's origin or follow another chain from it.

Both registry and local-origin candidates have their configuration checked
before their logs. A closed tree (`share off`) or one without configuration
is excluded from automatic discovery without reading its log. The local walk
also stops there, so it cannot reach a different tree above that boundary.
No configuration is regenerated and no sharing setting is changed.

Local-origin discovery inspects at most 64 repositories and 64 source
candidates. Each Git or tree configuration and the Git parser's decoded
output are limited to 64 KiB. Readable local-source logs share a 64 MiB budget,
with the existing 512 KiB line and 1,024 JSON item limits. Registry matches
only need their first complete event, limited to 512 KiB, after the permission
check.

Unreadable or invalid registry candidates are omitted. A local tree that
cannot be checked because of invalid configuration, linked or unreadable
metadata, an incomplete readable log or an exceeded bound refuses the check
instead of accepting a partial result. Event and lane metadata behind a
closed or missing configuration is not inspected. Candidates and the registry
are not changed, including during `--dry-run`; absolute source paths and
configuration values are not printed.

A remote URL cannot reveal another clone's local path. With no registry match
or usable local reference, `init` retains its existing uncertainty warning;
an empty registry still allows the first product to be created. This check
does not prove global uniqueness.

Scripts that previously created another tree from a locally referenced source
may now be refused. Select `--join` or `--new-tree` explicitly before retrying;
`--yes` confirms the plan's writes and does not select a separate product.
Because a previously successful scripted `init` can now be refused, this
change requires a minor release under the [versioning policy](VERSIONING.md).
It does not change the event format.

---

## What it writes, and where

Claude Code reads its settings and its MCP servers only from the folder it was
opened in, not from the folders above, so that is where setup writes them. The
tree is the `.vivac/` it finds going up from there, and `init` is what put it
there. If you open the agent in more than one folder of the same project, run
both commands in each: `init` makes that folder a thread of the same tree, and
setup gives it the hooks and the server. The plan names every folder before
anything is written.

| File | What it gets |
|---|---|
| `.claude/settings.json` | `UserPromptSubmit` runs `vivac session guard --hook` to report an unverified opening receipt without blocking work, then `vivac session prompt --hook` for custody and capture reminders; `Stop` runs `vivac session end --hook` |
| `.claude/skills/vivac-opening/` | the experimental opening module: `.claude-plugin/plugin.json`, `hooks/hooks.json`, `hooks/register.mjs`, and an ownership receipt, `.vivac-owned.json` |
| `.mcp.json` | the server, which runs `vivac mcp` |
| `.claude/skills/vivac-migrate/` | the skill an agent follows to bring another record into the tree — see [Migrating](MIGRATING.md) |
| `.claude/skills/vivac-agents/` | selected agent transfers and synchronization — see [Agent custody](AGENT-CUSTODY.md) |
| `.claude/skills/vivac-artifacts/` | selected existing deliveries and reviewed revisions — see [Artifact custody](ARTIFACTS.md); setup does not capture files |

Everything goes into the project and nowhere else. `.vivac/` is not on the
list: the tree is `vivac init`'s, and setup neither writes it nor removes
it.

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
anything that is not exactly its own. It removes intact owned opening files
and their empty private folders, preserving extra files and shared parents.
An edited opening bundle stays in place, with a warning that it may still run.
Custom opening or guard commands also remain, with a warning. `.claude/`
stays, even empty, because setup cannot tell whether it was there before.
The tree is never part of it: what `init`
wrote, `vivac init --undo` takes back. Right after planting, while the tree
holds no work yet, that is the whole `.vivac/` and its line in this
machine's registry. Once the tree holds work, `init --undo` never removes
it, and says so.

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

### Claude Code opening context

The module prepares the complete opening text once for `startup`, `clear`,
`resume` or `compact`, using `vivac session start --hook --context claude-module`.
It supplies that snapshot through an owned native attachment, preserving
foreign hook text. Preparation is limited to 60,000 UTF-8 bytes; oversized or
failed preparations supply no Vivac payload rather than a shortened opening.
The independent prompt check reports a missing or invalid module status and
allows native work to continue. Messages, delegation and resumed agents retain
their native behavior; Vivac does not guarantee that the same resumed agent
receives the latest project context.
This transport budget does not establish the active model's context capacity.

Re-run setup to upgrade the original plain-text opening hook. It removes only
exact `vivac session start --hook` command handlers, including those in groups
with foreign handlers. Custom opening or guard commands require review and
prevent an automatic upgrade. Bundle contents must match a compiled version
and its receipt; unknown or edited ownership is not permission to overwrite.
All planned configuration and bundle changes share one transaction. Updating
the engine alone does not replace an installed opening module: re-run setup
and restart the session to load the current bundle. Earlier owned bundles are
recognized for upgrade, including versions that blocked native continuations.

`vivac doctor claude-code` checks bundle integrity and the guard, prompt and
stop hooks. Configuration and an opening receipt do not prove that the
harness loaded the module or delivered its output. The native checks and their
limits are recorded in [Checking instruction delivery](INSTRUCTION-DELIVERY.md).

## Codex

```sh
vivac setup codex
```

The same pieces, in the three places Codex reads inside a project:
`.codex/config.toml` gets the server, `.codex/hooks.json` gets `SessionStart`,
`SubagentStart`, `UserPromptSubmit` and `Stop` running the session commands, and
`.agents/skills/` gets the same `vivac-migrate`, `vivac-agents` and `vivac-artifacts` workflows.
Nothing goes in your own configuration directory.

It refuses where there is no tree, and that refusal is the same argument
that used to make setup plant one: a project with integration files and no
tree has hooks that exit 0 in silence for ever, and nobody finds out.
Leaving that behind is not a cheaper setup, it is setup undone. So the
requirement stands and only the means changed — it names the `vivac init`
to run, and writes nothing until the tree is there.

It merges, and it can be taken back, the same way the Claude Code side does
and by the same rules: it adds to a file rather than replacing it, keeps
every key it does not own where it was, refuses a file it cannot read and an
entry under its name that it did not write, and a second run finds nothing
to do. `vivac setup codex --undo` removes exactly what it wrote and leaves
anything that is not exactly its own, `.codex/` and `.agents/` included; the
tree is never part of it.

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
finishes. Codex reads nothing under a project's `.codex/` until the project
is trusted, which lives in your own configuration, not the project's. The
first time Codex opens the folder it asks: say yes. If it does not ask, setup
prints the lines to add to `~/.codex/config.toml` yourself. And every hook is
approved on its own, against its hash, with `/hooks` inside Codex: the first
time, and whenever setup upgrades a handler or adds the subagent handler.

**When the project's instructions live in `CLAUDE.md`**, with no `AGENTS.md`
anywhere from the folder up to the repository root, setup says that too.
Codex reads `CLAUDE.md` only when its own configuration names it as a
fallback, so what the file says may never reach a Codex session. setup only
looks at which files exist and never opens them. It does not offer to write
an `AGENTS.md`, because a second copy drifts away from the first. What governs
the project belongs in the tree, which the brief hands to every agent, and the
vivac-migrate skill is what brings it in. The reverse needs no warning: since
version 2.1.277, Claude Code reads `AGENTS.md` on its own when there is no
`CLAUDE.md`.

Codex's session and subagent opening handlers run
`vivac session start --hook --context codex`.
It returns one JSON response containing the complete opening text and custody
notices. Setup makes the handler synchronous and sets `additionalContextLimit`
to `0`, avoiding Codex's default saved-file preview. The producer enforces a
separate ceiling of 60,000 UTF-8 bytes for that context. If the complete text
cannot fit, it returns `continue: true` with an explanation and no context,
rather than a truncated opening. Native work remains available. This ceiling
is a vivac transport budget, not a promise
about the active model's context window or account access.

Run setup again to review an upgrade from the original plain-text opening hook;
the changed handler must also be approved in Codex. Setup preserves foreign
hooks and reports custom commands or execution settings for review. The brief's
1,500-token budget remains soft: its fixed sections survive even when they
exceed it. Configuration and successful emission do not prove reception.
Check received context before replacing instruction files. See
[Checking instruction delivery](INSTRUCTION-DELIVERY.md).

`SubagentStart` prepares fresh context for the native child identifier and
delivers it as extra developer context, without copying the parent's
conversation. Re-run setup on an existing three-hook installation to add this
handler, then approve it in Codex. On the tested Codex CLI 0.163.0, rejecting
preparation does not prevent the child from starting. Doctor distinguishes a
correct handler definition from unverified reception and reports this consumer
limit; setup does not install a tool guard or promise to stop such a child.

setup takes three flags, and they read the same on both sides: `--dry-run`,
`--yes` and `--undo`. The four that decide where the tree lives — `--join`,
`--new-tree`, `--name` and `--lane-name` — are `vivac init`'s, because
which tree a folder belongs to has the same answer wherever the agent is
opened. Typing one of them at setup says so and names the command to run,
rather than failing as an unknown flag.
See [where it is measured](../README.md#where-this-is-measured).

`Stop` runs on every turn rather than once at the end, so the last stop does
not depend on the session closing cleanly. The stop is only saved if the tree
changed since the previous one: a stop that repeats identically is not a stop,
it is a log.

The hook prints nothing, so a stop you expected and did not get looks the same
whatever the cause. `vivac session end --dry-run` tells them apart without
writing anything. It says what closing would do now, and what the hook decided
the last time it ran in this folder: the stack was empty, nothing had changed,
another write held the tree, or which stop it wrote. With no record at all,
the harness did not run it, which is what a hook still waiting for approval
looks like. The hook leaves that verdict in a small file next to the clock
described below, never in the log.

`UserPromptSubmit` runs on every message you send, and almost always says
nothing. It speaks only when the agent has worked ten minutes, added up
across its turns, without writing anything to the tree. The time you take to
answer does not count: `Stop` closes each turn's clock and the next message
opens it again, so an agent that wrote something and then waited an hour for
you is not told it has gone quiet. When it does speak, the agent reads one
line saying how long it has worked, and that anything that happened since, a
choice, a finding, work done, goes in the tree before it answers. Having
spoken, it keeps quiet for ten minutes. It never blocks your message and
never writes to the log; the clock lives in a small file under the system's
temporary directory. Long sessions are where the seams
fade: the brief arrives when a session opens, not in the turn where the work
happens.

The ordinary plain-text start hook, prompt and end commands stay quiet and
exit 0 where there is no `.vivac/`. Claude's module producer instead returns
JSON with `status: absent`. Its independent guard checks the module status: a
valid `absent` status is silent, while a missing or invalid status adds an
unverified-context warning. Both permit native continuation.

---

## Any other harness

What the hooks call is `vivac session start`, `vivac session prompt` and
`vivac session end`, which are commands like any other. `--hook` makes them speak to a harness instead of
a person: the brief goes out as plain text, and what kind of start it was is
read from what the harness passes in. So each can be run by hand to see
exactly what a hook would do. `session end` by hand writes the stop it finds
due, the same as its hook; `--dry-run` looks without writing.

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

**Fifteen tools.** Five are reads: `vivac_brief`, `vivac_find`, `vivac_why`,
`vivac_open` and `vivac_rules`. Ten are writes: `vivac_push`, `vivac_pop`,
`vivac_done`, `vivac_add`, `vivac_decide`, `vivac_note`, `vivac_park`,
`vivac_save`, `vivac_arm` and `vivac_declare`. The server speaks JSON-RPC over
standard input and adds no dependency: it is the binary you already installed.

Fifteen and not more, because every tool costs context in every session the
agent ever opens, so **the list is a budget and not a catalogue.** Eight of
the writes are the seams of the work: opening something, closing it, closing
a record or something finished elsewhere, parking it, noting it, deciding, and
the safe stop. The other two are the seams of governance: arming a rule with
the command that checks it, and declaring what a decision was judged against.
Nothing else got in. `vivac_done` never closes over open closure conditions,
and nothing that discards a node is a tool: those stay at a terminal, where a
person is looking.

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

**A project can keep what it knows to itself.** Work for one client has no
business turning up in the session of another's. `vivac share off`, run in
that project, closes it: from every other project, `find --everywhere`,
`vivac_find` and `check --gates` leave it out and say only how many they left
out, and
`--project` and `vivac_why` refuse to open it. From inside the project itself
nothing changes. Projects start open; `init` says so when it plants a tree.

Closing asks nothing, since it can only keep more in. Reopening, `vivac share
on`, asks a person at a terminal and takes no `--yes`, so an agent running
commands cannot reopen what somebody closed, and neither command is a tool.
That guards the command, not the file: the mark is one field in
`.vivac/config`, and whoever can edit that file can change it. Deleting it
does not reopen the project. vivac rebuilds a missing config from the log
closed, with a new project id, and says so, because whether the project
shared lived only in the file that is gone: opening by mistake is a leak
nobody can take back, and closing by mistake is one `vivac share on` away.
Until the project itself rebuilds it, every other project reads it as
closed, and a read from elsewhere never writes it. A vivac older than
0.17.19 rebuilds it open, and one older than 0.17.7 does not know the field
and reads a closed tree like any other.

### Nothing destructive is reachable from here

`abandon` discards a node and everything below it, and through a tool that
would happen without anybody seeing a command. It stays on the command line,
where somebody is looking. So do the operations that reshape a tree rather
than record work — closing over open closure conditions, blocking, flagging,
restoring a safe point. Those belong to whoever maintains the tree, and
they have a terminal.

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
> **On Windows, run `vivac update` before updating.** A running `vivac mcp`
> holds the executable open, so `cargo install vivac` cannot replace it and
> fails with an access-denied error — *os error 5* — that names neither MCP
> nor this command, and so does not lead back to the cause. `vivac update`
> sets the running copy aside and, once you answer yes, installs the new
> release itself; the sessions already open keep the old version until they
> restart. Linux and macOS replace a running binary without complaining.

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
