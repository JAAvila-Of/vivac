# Bringing a project in

> **Nothing moves into vivac on its own.**

If a project already keeps what it has learned — in a memory or learning
system, in `CLAUDE.md`, `AGENTS.md`, `MEMORY.md`, the harness's own memory, or
internal documents — **none of that is in the tree once vivac is set up.**
vivac never reads another system, and never reads those files.

That is not an omission waiting to be fixed. Telling a rule from the prose
around it takes judgment, and a tool that guessed would fill your tree with
confident nonsense on day one. So bringing them in is a job for the agent,
with you deciding what goes in, and setup installs the `vivac-migrate` skill
that walks it through: where to look, how to sort what it finds, how to check
what it wrote, and how to retire the other maps afterwards.

---

## Why this is a migration and not an addition

The short answer is in the README: [**one map**](../README.md#one-map). Two
records of the same work do not add up — each one points the agent at the
context it holds, and sooner or later one settles something the other mapped
differently, without anybody noticing which of the two oriented the decision.

Which means the job is not finished when the tree is full. It is finished when
**the other maps stop talking to the agent.** The skill treats that as the last
step rather than an afterthought, and it is the step people skip.

vivac itself does not turn anything off, and neither does setup: another
system is not vivac's to touch. The skill finds every other map the agent
receives — from a memory tool's plugin to lines in an instruction file that
tell the agent to save somewhere else — and at the end offers to retire each
one for this project, in every folder you open the agent in. Your agent takes
each step only after you say yes to it, in a form that can be undone, and
**never deletes another system's data or uninstalls it.**

---

## The steps

**1.** Install vivac and set the project up, in the folder you open the agent
in:

```sh
cargo install vivac
vivac init
vivac setup claude-code
```

`init` plants the tree, `setup` gives the agent the hooks, the server and
this skill. See [Setting it up](SETUP.md) for what each one writes.

**2.** Open a new session in the project. If it asks whether to use the
`vivac` server, say yes.

**3.** Ask the agent:

> Use the vivac-migrate skill to bring everything this project knows into
> vivac.

It lists every source it finds, from a memory system to the harness's own
memory, instruction files and internal documents, and asks which to bring in.
It shows you a plan before writing anything, checks what it wrote, and then
offers to retire the other maps, one at a time.

Until then, another record you use keeps talking to the agent as before, and
may tell it to use that one first. That is expected: the skill only reads from
it.

**4.** Open a fresh session. The brief it starts with is what the tree now
knows.

---

## What lands where

**What has to hold in every session goes in as a constraint under the root
goal**, which the brief hands the agent every time.

Pillars and rules are read on demand, with `vivac rules`, when work is
checked — not injected into every session, because a rule that arrives unasked
in a thousand sessions costs a thousand times what it costs to fetch it in the
one where it matters.

Instruction files stay as they are for now. What they say still reaches every
session from the file, and taking that away before the tree delivers it would
trade one gap for another.

---

## Doing it a second time

A folder joined to a tree that already exists brings its own history with it:
its own instruction files, its own memory, its own documents — and none of that
is in the tree yet, however full the tree already is.

The skill is built for that case. Its second step looks at the tree first and
proposes a note on the node that already says it, rather than writing a
duplicate.
