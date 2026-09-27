---
name: vivac-migrate
description: Bring everything a project already knows into the vivac tree, from another memory system, the harness's own memory, instruction files for any agent, decision records and internal documents; check it, and retire the other maps with the person's yes. Use when asked to migrate, import or move memories, notes, rules, decisions or documentation into vivac, or to bring what a project knows into it.
---
<!-- written by vivac setup; fingerprint a6415fc3dfd585a8; setup removes it with --undo while the text is unchanged -->

# Bringing what a project knows into vivac

vivac never imports anything by itself. It does not read other memory systems,
the harness's own memory or any instruction file. Moving what they hold into
the tree is your job, and the person decides what goes in.

The tree's log only grows: what you write stays written. So nothing is written
until the person has seen the plan and said yes. Everything you need to do this
is on this page; you do not need to read vivac's source.

A migration is judged by one thing: whether what each source knew can still be
found in the tree afterwards. Counting what you wrote does not show that. Step 5
measures it, source by source, and the work is not done until it passes.

## Ground rules

- Never write a secret, a credential, an email address, a person's name or a
  path inside someone's home directory. The redaction guard refuses some of
  these and exits with 3. When it does, write the node again without the
  value, and never work around the guard.
- Never copy a file's contents or code into a node, not even one line: say in
  words what the code does, and name the files, types and functions involved.
  Write what was decided or learned and why, and point at files in the
  project with --ref.
- Never point a node back at the other system: no memory ids, topic keys or
  links into it. Once it is retired they lead nowhere, and until then they
  send the next agent to a second map.
- Do not change another memory system, the harness's memory or any
  instruction file before step 6, and there only one step at a time, each
  after the person says yes to it.
- Read other tools with their read and search commands. Do not guess their
  flags, and do not add one to a command that writes, not even --help: some
  tools take it as an argument.
- Anything you write to work with, such as an export, a plan too long for the
  conversation, a script or a list of sources, goes in a temporary folder
  outside the project and outside .vivac/. An export is a copy of everything
  the tool holds, for every project. Delete the export before you ask the
  person anything, since they may stop there, and delete the rest when the
  migration ends. Nothing of yours stays behind in the project: a plan or an
  inventory left there is one more map, and nobody keeps it current.
- Do not use vivac import. It reads trees from vivac's own prototype, not
  memories, and it changes kinds on the way in.

## 1. Take stock

Find every place this project's knowledge lives before proposing anything.
Then show the person what you found, with what each source holds and how big
it is, and ask which to bring in.

- What the harness gave you when this session opened. Your context says where
  each part came from: instruction files for this project or for the user,
  the harness's own memory, and whatever hooks printed. Anything there that
  tells you to save or look things up somewhere other than vivac is another
  map: note where it came from, for step 6. A file the user keeps for every
  project, like a CLAUDE.md in their home directory, is not this project's.
  Do not bring it in: it already reaches every session.
- Memory systems. If a memory tool is installed, read it with its own tools:
  the one that recalls what is current, the one that searches, and the one
  that fetches a single record whole, because search results usually come
  back truncated and the summary is not the record. If its tools are off in
  this session, its command line reads the same memories. Keep only what
  belongs to this project. Memory tools often file a project under the name
  of the folder a session was opened in, so look for it under every name it
  may have had, like a repository inside it, and for memories filed under
  another project by mistake.
- The harness's own memory for this project. In Claude Code, that is the
  MEMORY.md it loads every session and the files it points to, and the
  memory its subagents keep under .claude/agent-memory/. Claude Code keeps it
  in the user's home, under a folder named after the project's path, and an
  agent in another harness can read it there too. In Codex, memory is a
  feature the user turns on, and it is kept for the user rather than for one
  project, under the memories folder in Codex's home; if it is there, keep
  only what belongs to this project.
- Instruction files for any agent, anywhere in the project, including the
  repositories inside it and their own .claude folders: CLAUDE.md,
  CLAUDE.local.md, AGENTS.md, GEMINI.md, .cursorrules, .cursor/rules/,
  .github/copilot-instructions.md, .windsurfrules, .clinerules,
  CONVENTIONS.md and CONTRIBUTING.md.
- Decision records and internal documents: adr/, decisions/, docs/, design/,
  notes/, a wiki folder, and any other text that states rules, decisions or
  constraints. Skip vendored, generated and dependency folders.
- Services you can reach from this session that are not a memory system,
  like a drive, a wiki or an issue tracker behind a connector. They hold far
  more than this project, so do not search them on your own: ask the person
  whether this project keeps anything there, and search only what they name.

Read whole what the person picks. If a long document is mostly finished steps
or code, say so and read its prose.

Count what you found, in records, files and size, and tell the person. When it
runs to hundreds of records or more, say that the migration will take a while
and will go in batches: first the structure, meaning the root goal, pillars,
constraints, rules and decisions, and then what was learned. Each batch has its
own plan, its own yes, its own copy of the log and its own check. The second
batch is not optional: it is where most of a large project's knowledge is.

If your harness can start subagents, split the reading of that second batch
among them, a share of the sources each, so thousands of records do not fill
your context. The first batch is not split: the structure needs the whole
project in view. Give each subagent its sources, step 3 of this page and the
ground rules. It only reads: it writes nothing to the tree and changes nothing
anywhere. It answers with one line for every source it was given, either the
nodes that source would become, each with its kind, title, parent and why, and
two or three terms only that source would use, for step 5; or that nothing
comes out of it, and why. A summary instead of those lines is not an answer:
whatever it left out is lost where nobody can see it. Before you use what comes
back, check that every source you handed out has its line, and read again any
source that does not. The plan, the person's yes and the writing stay with
you. Without subagents, do the same reading yourself.

## 2. Look at the tree

The tree may already hold some of this, from an earlier migration or from
work. Read vivac brief and vivac rules. Before proposing a node, look with
vivac find for one that already says it, and if there is one, propose a note
on it instead.

## 3. Propose

Sort every item into one kind. Where a node goes decides whether it reaches the
next agent on its own:

| Kind | What it is | How it reaches the agent |
|---|---|---|
| constraint | Something that has to stay true. | Under the root goal: in the brief, every session. Under another node: only while the work is inside it. |
| pillar | A criterion the project's design is judged against, titled with its name and what it rejects. | vivac rules, when work is checked. |
| rule | A line a pillar draws that work can be checked against, with the command that checks it if there is one (--arm). | vivac rules, when work is checked. |
| decision | A choice that was made, with its reason, the options it beat, and the pillar or rule it was judged against. | The brief shows a few that still stand; vivac why shows the rest. |
| finding, still open | Something observed that still asks for work: a bug nobody fixed, a gap, a risk. | vivac open, and the brief. |
| finding, closed as a record | A lesson or a measurement that asks nothing of anyone. Write it and close it at once, with an outcome that says it is a record. | vivac find and vivac why, which include closed nodes. |
| question | Something still to decide, including whatever a source marks as proposed, draft or pending. | The brief shows the ones that block. |
| goal or task | Work still open. | vivac open, and the brief. |
| nothing | Status, superseded history, boilerplate, code, and descriptions of how the system works. | It stays in its source. |

- What has to hold in every session, whatever the work, is a constraint under
  the root goal. How to work in the repository usually is. What is only
  judged when a piece of work is reviewed is a rule under its pillar.
- A lesson that governs how work is done is a rule or a constraint, not a
  finding: that is where it reaches the agent. A lesson that only records
  what happened is a finding closed as a record. Leaving lessons open fills
  vivac open with things that are not work, and then it stops saying what is.
- A pillar comes from the person's own words, or from a document that calls it
  a pillar or a governing criterion. If you think something works as one but
  nobody named it, propose it marked as inferred, and for each one show what
  it would reject in practice and whether it holds for the whole project. One
  that only holds for one part of the work is a rule under that part's goal.
  Let the person decide. Whatever you found, the plan says it: the pillars
  the sources name, the ones you infer, or that there are none. Rules with no
  pillar above them are allowed, but the person has to have seen that.
- A description of how the system works is not a rule, however important it
  is. A rule is a line that work can be checked against; the description stays
  in its document.
- Every decision names the pillar or rule it was judged against, with a
  sentence on how it holds (--against). That sentence is not the reason: it
  says what the decision does to satisfy that pillar or rule. Once the tree
  has pillars, vivac check flags a decision that does not name one. If none
  applies, tell the person.
- When two sources disagree, ask which one holds before writing either.
- Something that already has its own register in the project, like a list of
  bugs or a coverage file per object, is referenced, not copied a node per
  entry: group it the way the register does, with --ref to the register. A
  copy in the tree would be a second register, and it would drift.
- Several sources that say the same thing become one node. Merging must not
  lose what made each source worth keeping: the concrete mechanism, the tool,
  the object and the symptom. A lesson whose point is a mechanism gets a node
  of its own, not a sentence inside a broader note.
- Status and "where we are" do not move: the tree is the state, and vivac
  brief, vivac open and vivac parked answer that. Superseded history stays in
  its source. Links become --ref.

If the tree has no root goal, propose one that says what the project is for.
Decisions about the whole project hang from it, a pillar's rules hang from the
pillar, and what belongs to one part of the work hangs from that part's goal.
When unsure, ask.

Show the person, for each source, how many nodes come out of it and what is
left out and why. Then a table per kind, with the source, the title, the parent
and, for each decision, its --against. Then the other maps you found in step
1. Wait for their answer before writing.

A yes to the sources and how you will treat them is not a yes to the nodes.
Each batch shows its own tables, title by title, and waits for its own yes,
even when the person already approved the plan or asked for only one batch.

## 4. Write

1. Copy .vivac/events to .vivac/events.pre-migration, or to another name if
   an earlier batch took that one, and check both are the same size. One copy
   per batch. Putting that copy back, with every session that runs vivac mcp
   closed, is how a batch is undone.
2. Write the root goal if it is new, then pillars and constraints, then rules
   under their pillar, then decisions, then findings and open work. Use vivac
   add and vivac decide on the command line, or vivac_add and vivac_decide
   over MCP. Give each decision the options it beat with --alternative, and
   what it was judged against with --against. Close each record right after
   writing it, with vivac done and its outcome on the command line.
3. Before giving a rule a command with --arm, run the command once. Attach it
   only if it passes and actually checks something, and tell the person which
   rules were left without one.

## 5. Check

- For every source, not a sample, take two or three terms that only it would
  use: the name of an object, a tool, a command or a symptom. Search each one
  with vivac find, which ignores accents. List every source for which none of
  its terms finds anything. For each one, write the node it was missing, or
  write down why the tree already says it under other words. The migration is
  not done while any source on that list has neither.
- Compare what was written with the plan, not with itself: every title,
  parent and --against sentence against the plan the person approved.
  Explain every difference.
- vivac check comes back clean.
- vivac rules lists every pillar and rule you wrote.
- vivac brief shows the constraints under the root goal.
- vivac open shows work, not records.
- Look through the sources for sentences that now send a reader to the old
  place, like "read X first" or "save this to Y", and for statements the tree
  now contradicts, and list them for the person. Do not edit them.
- Delete any export you made.

Then tell the person what was written, what was left out and why, and which
copies of the log you made.

Last, ask the person to name three or four things this project learned the
hard way, the ones they would least like to lose, and search for each in front
of them with vivac find. No automatic check tells you whether the tree answers
what they remember. If one is missing or reads too thin to be recognised, fix
that before going on.

## 6. Retire the other maps

Start this only when step 5 has passed, the person's own search included.

Two maps collide: each one points the agent at what it holds, and sooner or
later one settles something the other mapped differently. For each map you
found in step 1, tell the person what it is, where it lives, and the exact step
that stops it reaching this project. Take each step only after they say yes to
it, and in a form that can be undone:

- A memory tool's plugin: turn it off for this project only. In Claude Code,
  that is "enabledPlugins": { "<plugin>": false } in .claude/settings.json,
  with every other key left as it was.
- The harness's own memory, and each subagent's: freeze its index. Copy
  MEMORY.md to MEMORY.md.pre-freeze beside it, then replace it with a short
  note saying that the project's knowledge now lives in the vivac tree and how
  to read it. Leave the files it pointed to where they are. If it has no
  MEMORY.md yet, write the note anyway: otherwise the first memory an agent
  saves there starts a new index.
- Codex's memory belongs to the user, not to this project. Turning it off
  changes every project they open in Codex, so say so and leave that choice
  to them.
- Lines that tell the agent to save or search somewhere other than vivac, in
  any instruction file: show them and say where they came from; an installer
  often marks its blocks. Keep a copy of the file beside it and remove only
  those lines. In a file the user keeps for every project, say that the change
  reaches every project, not just this one.
- Everything else in this project's instruction files stays as it is. Their
  rules still reach every session from the file, so list the ones the tree now
  repeats.

In Claude Code, the plugin setting and the harness's own memory belong to the
folder a session is opened in: it reads its settings only from that folder,
and keeps a separate memory for each repository. If the person may open
Claude Code in another folder of this project, like a repository inside it
with its own .claude folder, or one a memory system files memories under,
ask them. In each folder they do, offer those steps there as well, and offer
to set vivac up there, so that sessions opened there get the brief. That is
two commands in each folder, and both ask before writing: show them the plan
that vivac init --dry-run prints there, and run vivac init --yes only after
they say yes, then do the same with vivac setup claude-code. The first makes
the folder a thread of this same tree; the second gives that folder the hooks,
the server and the skill.

Never delete another system's data and never uninstall it: whether it keeps
running for other projects is the person's call.

When everything is done, check that nothing of yours is left in the project,
remind the person of the copies of the log in .vivac/, and delete the ones
they say to delete. Then ask them to open a new session: the brief it starts
with is what the tree now knows.
