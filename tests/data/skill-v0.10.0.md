---
name: vivac-migrate
description: Bring what another memory system or a project's own files know into the vivac tree, and check that nothing was lost or leaked on the way. Use when asked to migrate, import or move memories, notes, rules, decisions or documentation into vivac, from engram or any other memory tool, or from CLAUDE.md, AGENTS.md, MEMORY.md or internal documents.
---
<!-- written by vivac setup; fingerprint 11ae0cbfc614c228; vivac setup claude-code --undo removes it while the text is unchanged -->

# Bringing another memory into vivac

vivac never imports anything by itself. It does not read other memory
systems, and it never reads CLAUDE.md, AGENTS.md or any memory file. Moving
what they know into the tree is your job, with the person deciding what goes
in.

The tree's log only grows: what you write stays written. So nothing is
written until the person has seen the plan and said yes.

## Ground rules

- Never write a secret, a credential, an email address, a person's name or a
  path inside someone's home directory. The redaction guard refuses some of
  these. When it does, write the node again without the value, and never
  work around the guard.
- Never copy a file's contents or code into a node. Write what was decided or
  learned and why, and point at files by path with --ref.
- Do not turn the other system off, and do not edit or delete anything in it.
  Whether it stays on is the person's call; the vivac README says why running
  two memory systems side by side is discouraged.
- Keep CLAUDE.md, AGENTS.md and MEMORY.md as they are. vivac does not hand its
  rules to the agent when a session opens yet, so the file is still what
  delivers them unasked.
- Do not use vivac import. It reads trees from vivac's own prototype, not
  memories, and it changes kinds on the way in.

## 1. Keep a copy of the log

Before the first write, copy .vivac/events to .vivac/events.pre-migration and
check that both are the same size. Putting that copy back, with every session
that runs vivac mcp closed, is how a migration is undone.

## 2. Take stock

List every source and count what it holds.

- A memory system: read it with its own tools. With engram, that is
  mem_context, mem_search, and mem_get_observation for each hit, because
  search results come back truncated. If its tools are turned off in this
  project, its command line or its export reads the same memories. Keep only
  what belongs to this project.
- Instruction files and internal documents: read them whole.

## 3. Propose

Sort every item into one of these:

- decision: a choice that was made, with its reason and the options it beat
- finding: something observed or learned that later work depends on
- constraint: something that has to stay true
- pillar: a criterion the project's design is judged against, titled with its
  name and what it rejects, in the project's own words
- rule: a line a pillar draws, hung under that pillar or under the root goal,
  with the command that checks it if there is one (--arm)
- goal or task: work that is still open
- nothing: superseded history, status tables, boilerplate, code

Status and "where we are" do not move: the tree is the state, and vivac
brief, vivac open and vivac parked answer that. Superseded history stays in
the source. Links become --ref.

Parents take judgment, because a source rarely says what hangs from what.
Decisions about the whole project hang from the root goal, and a pillar's
rules hang from the pillar. When unsure, ask.

Show the person a table with the source item, the kind, the title and the
parent, and a second list with everything left out and why. Wait for their
answer before writing.

## 4. Write

Pillars and constraints first, then rules under their pillar, then decisions,
then findings and open work. Use vivac add and vivac decide on the command
line, or vivac_add and vivac_decide over MCP. Give a decision the options it
beat with --alternative.

## 5. Check

- Count what was planned against what was written, and explain every
  difference.
- vivac check comes back clean.
- vivac rules lists every pillar and rule you wrote.
- vivac brief shows the constraints and the standing decisions.
- vivac find finds a few distinctive words from each source. Try them with
  and without accents.
- Look through the sources for sentences that now send a reader to the old
  place, like "read X first" or "save this to Y", and list them for the
  person. Do not edit them.

Then tell the person what was written, what was left out and why, and where
the copy of the log is.
