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
vivac setup claude-code    # or vivac setup codex, or both
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

A project with a long history takes a while. With hundreds of memories or
more, the skill says so and works in batches: first the structure (the root
goal, pillars, constraints, rules and decisions), then what was learned, each
batch with its own plan and your own yes. Its working files stay in a
temporary folder, never in your project. If your agent can start subagents,
the reading of that second batch is shared out among them, and each one
answers for every source it was given, so nothing is dropped where you cannot
see it; the plan and the writing stay with the agent you are talking to.

Until then, another record you use keeps talking to the agent as before, and
may tell it to use that one first. That is expected: the skill only reads from
it.

A project that starts out with vivac has nothing to bring in, and still needs
the skill if another memory system is installed: that system keeps talking to
the agent in this project until something turns it off here. With nothing to
bring in, the skill says so and goes straight to retiring the other maps.

**4.** Open a fresh session. The brief it starts with is what the tree now
knows.

---

## How you know it worked

**Counting what was written proves nothing.** A merged summary can match the
plan node for node and still have lost the one detail that made a lesson worth
keeping. So the skill checks every source, not a sample: it takes the few terms
only that source would use, searches the tree for them, and does not finish
while any source comes back with nothing and no reason why.

Then it asks you for the three or four things the project learned the hard
way, and searches for each in front of you. That is the check no machine can
make, and the other maps are not retired until it passes.

---

## What lands where

**What has to hold in every session goes in as a constraint under the root
goal**, which the brief hands the agent every time.

Every active pillar's complete title arrives in the brief, even with no
focus. This section is fixed: a small token budget does not remove a pillar.
Read `vivac rules` before making decisions or checking work to get their
full reasons and rules. Rule bodies are not injected into every session.

A lesson or a measurement that asks nothing of anyone goes in closed, as a
record: `vivac find` and `vivac why` still bring it back, and `vivac open`
keeps answering what is actually left to do. That includes most of what a
project learned the hard way. A lesson keeps its mechanism and its symptom in
its title, and becomes a rule only when it draws a line that any piece of work
can be checked against; the lessons that taught a rule hang under it. A
migration that turns every lesson into a rule leaves a rule list nobody reads
at review, and loses the words the lessons would be found by.

Instruction files stay as they are for now. What they say still reaches every
session from the file, and taking that away before the tree delivers it would
trade one gap for another.

Agent configuration has a separate lifecycle. `vivac agents` lists project
agents across harnesses without importing them. Use `vivac agents sync` to
choose a source, destinations and explicit model and effort assignments,
then review the plan before applying. It preserves the complete native prompt
through file references; its body never goes into the tree. There is no need
to restate the prompt as prose or run import, set and bind separately.
See [Agent custody](AGENT-CUSTODY.md). Discovery and generation do not prove
that a harness loaded the configuration, or that the agent applies its rules.

---

## Doing it a second time

A folder joined to a tree that already exists brings its own history with it:
its own instruction files, its own memory, its own documents — and none of that
is in the tree yet, however full the tree already is.

The skill is built for that case. Its second step looks at the tree first and
proposes a note on the node that already says it, rather than writing a
duplicate.

---

## What happened on real projects

Everything above came out of bringing real projects in, eight times between
7 and 26 September 2026, all of them projects I work on. They are named by
letter: the point is what the migration did, not what the projects are.

| | When | Harness, version | What came in | What the tree ended with |
|---|---|---|---|---|
| **A** | 7 Sep | Claude Code, 0.6.0, by hand before the skill existed | a 131,269-token tracker, 1,520 memories, 86 harness memory files | 51 nodes; the brief went from the whole tracker to 689 tokens |
| **B** | 14 Sep | Claude Code, 0.10.0 | 119 memories, the harness memory, internal documents | 145 nodes, then 220 with the documents; a brief of 584 tokens |
| **C** | 15 Sep | Claude Code, 0.11.0–0.11.1 | a 36 KB instruction file, six plans, 66 memories | 101 nodes, a brief of 507 tokens |
| **D** | 15 Sep | Claude Code, 0.11.1–0.11.2 | its memory | 37 nodes |
| **E** | 21 Sep | about 0.12.3 | about 40,000 lines across six instruction files, 529 memories | the inventory was made; what it wrote was not recorded |
| **F** | 23–24 Sep | Codex, 0.14.2, guided step by step | 1,556 memories, 306 harness memory files, 3,442 documents | 462 nodes in about an hour and a half |
| **F, test 1** | 26 Sep | Claude Code, 0.15.9, no guidance | the same project's 305 lesson files and its instruction files | 352 nodes, 296 of them rules |
| **F, test 2** | 26 Sep | the same, with the corrected skill | the same | 480 nodes, 32 rules |

The project that builds vivac is not in the table: its harness memory held
nothing the tree did not already say, and its memory tool was not imported at
all. Of 5,868 of that tool's memories, the redaction guard would have refused
264 — 199 for a path inside a home directory, 63 for an email address, two for
looking like a secret. That is the case against an importer in one number.

**The two tests ran on the largest of these projects**, on purpose: a large
project is where a migration loses things. The smaller ones were shorter, and
in my experience they held up better, but I did not check them source by
source, so there is no number for them here.

### The two measured tests

Both ran in a copy of project F, from the same starting point, with the same
prompt and no guidance, on a model of ordinary cost. Afterwards every one of
the 305 lesson files was checked against the tree: first by script, on three
terms only that file uses, then by hand wherever the script was unsure.

| | Test 1 | Test 2 |
|---|---|---|
| The agent's working time | about 50 minutes | about 44 minutes |
| Rules written | 296 | 32 |
| `vivac rules`, read at every review | 43.5 KB | 4.9 KB |
| Lessons confirmed in the tree | about one in five for certain | **286 of 305** |
| Lessons with no trace | **at least one in four** | 4 |

Test 1 turned almost every lesson into a rule, and a lesson rewritten as a
general line loses the mechanism and the symptom it would be searched by. The
skill also asked for a check on every source, and the agent ran twenty
searches instead. Test 2 kept lessons as records titled with their mechanism,
and ran the check on every source; of the four it lost, three describe the
tool being replaced or the agent's own habits rather than the project. Test 1's number is
a floor: its uncertain half was never resolved one by one.

### What each failure changed

- **Decisions went in without what they were judged against**, and the check
  failed at the end. The plan asks for it now (0.11.0).
- **The other memory system kept talking during the migration.** In one
  session the agent made 88 calls to it and wrote nothing to the tree. The
  skill says so before starting (0.11.1).
- **The agent searched a connected drive without asking.** It asks first
  (0.11.2).
- **An export of every project the memory tool held, 22 MB, stayed on disk.**
  It is deleted before anything is asked (0.11.3).
- **A search missed a word written with an accent.** `vivac find` ignores
  accents (0.12.1).
- **Condensing a large batch lost three sources in four.** The skill works in
  batches, checks every source and keeps its working files out of the project
  (0.15.2).
- **Thirteen decisions were superseded only to fix one sentence each.**
  Declaring again replaces the sentence (0.15.2).
- **One agent reading thousands of sources compacted its context five
  times.** The reading is shared among subagents, each answering for every
  source it was given (0.15.5).
- **Test 1's failures**: lessons become records, the check runs on every
  source and is shown to the person, every source found is asked about, a
  reading subagent publishes nothing, a batch too large for the conversation
  goes to a file, and the last check stays with the person's memory (after
  0.15.9).
- **A project that started out with vivac never ran the skill**, since it had
  nothing to bring in, and its memory tool's plugin told the agent to use it
  first in every message, three sessions running, until it was turned off by
  hand. Setup says the skill is for that project too, and with nothing to
  bring in the skill goes straight to retiring the other maps (after 0.17.22).

### What still holds

- **There is no importer, and there will not be one.** Every migration is
  judged by an agent with the person deciding, once per project.
- **vivac does not turn another system off.** If it stays, the agent has two
  maps, and the skill can only say so.
- **A tree migrated before 0.15.1 costs more to find your way in.** In
  project A, one question about where things stood took the agent about
  twenty calls, fourteen after closing the records it had left open; in the
  others it took five at most. Either way it beats rereading the project.
- **Reading a memory tool's export shows the agent other projects' memories.**
  The skill deletes the export, but it reads it first.

### What it costs

Test 2 took about 44 minutes of the agent's time and some 16 dollars at API
prices, on a model of ordinary cost. The waiting on the person is not
counted: one of the tests sat 37 minutes on a question during lunch. An
earlier repeat of project F in Codex was stopped halfway because finishing it
would have taken another fifth of a weekly allowance: it ran the most
expensive model at high effort, on a volume where that was the wrong choice.
