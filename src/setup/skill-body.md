
# Bringing what a project knows into vivac

vivac never imports anything by itself. It does not read other memory systems,
the harness's own memory or any instruction file. Moving what they hold into
the tree is your job, and the person decides what goes in.

The tree's log only grows: what you write stays written. So nothing is written
until the person has seen the plan and said yes. Everything you need to do this
is on this page; you do not need to read vivac's source.

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
  tools take it as an argument. An export is a copy of everything the tool
  holds, for every project. If you need one, write it to a temporary folder
  outside the project, and delete it before you ask the person anything:
  they may stop there, and then nothing would delete it.
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
  this project, its command line reads the same memories. Keep only what
  belongs to this project. Memory tools often file a project under the name
  of the folder a session was opened in, so look for it under every name it
  may have had, like a repository inside it, and for memories filed under
  another project by mistake.
- The harness's own memory for this project. In Claude Code, that is the
  MEMORY.md it loads every session and the files it points to, and the
  memory its subagents keep under .claude/agent-memory/.
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
| finding | Something observed or learned that later work depends on. | vivac why and vivac find. |
| question | Something still to decide, including whatever a source marks as proposed, draft or pending. | The brief shows the ones that block. |
| goal or task | Work still open. | vivac open, and the brief. |
| nothing | Status, superseded history, boilerplate, code, and descriptions of how the system works. | It stays in its source. |

- What has to hold in every session, whatever the work, is a constraint under
  the root goal. How to work in the repository usually is. What is only
  judged when a piece of work is reviewed is a rule under its pillar.
- A pillar comes from the person's own words, or from a document that calls it
  a pillar or a governing criterion. If you think something works as one but
  nobody named it, propose it marked as inferred, and let the person decide.
- A description of how the system works is not a rule, however important it
  is. A rule is a line that work can be checked against; the description stays
  in its document.
- Every decision names the pillar or rule it was judged against, with a
  sentence on how it holds (--against). Once the tree has pillars, vivac check
  flags a decision that does not. If none applies, tell the person.
- When two sources disagree, ask which one holds before writing either.
- Status and "where we are" do not move: the tree is the state, and vivac
  brief, vivac open and vivac parked answer that. Superseded history stays in
  its source. Links become --ref.

If the tree has no root goal, propose one that says what the project is for.
Decisions about the whole project hang from it, a pillar's rules hang from the
pillar, and what belongs to one part of the work hangs from that part's goal.
When unsure, ask.

Show the person a table per kind, with the source, the title, the parent and,
for each decision, its --against. Then a list of everything left out and why,
and the other maps you found in step 1. Wait for their answer before writing.

## 4. Write

1. Copy .vivac/events to .vivac/events.pre-migration, or to another name if
   an earlier migration took that one, and check both are the same size.
   Putting that copy back, with every session that runs vivac mcp closed, is
   how a migration is undone.
2. Write the root goal if it is new, then pillars and constraints, then rules
   under their pillar, then decisions, then findings and open work. Use vivac
   add and vivac decide on the command line, or vivac_add and vivac_decide
   over MCP. Give each decision the options it beat with --alternative, and
   what it was judged against with --against.
3. Before giving a rule a command with --arm, run the command once. Attach it
   only if it passes and actually checks something, and tell the person which
   rules were left without one.

## 5. Check

- Count what was planned against what was written, and explain every
  difference.
- vivac check comes back clean.
- vivac rules lists every pillar and rule you wrote.
- vivac brief shows the constraints under the root goal.
- vivac find finds a few distinctive words from each source. It matches
  accents exactly, so search the way the source spells them.
- Look through the sources for sentences that now send a reader to the old
  place, like "read X first" or "save this to Y", and for statements the tree
  now contradicts, and list them for the person. Do not edit them.
- Delete any export you made.

Then tell the person what was written, what was left out and why, and where the
copy of the log is.

## 6. Retire the other maps

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

Last, ask the person to open a new session: the brief it starts with is what the
tree now knows.
