# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.0](https://github.com/JAAvila-Of/vivac/compare/v0.13.0...v0.14.0) - 2026-09-23

### Upgrading

- **`vivac init` with no flags now does what its own plan says.** In `0.13.0`
  a bare `vivac init` still planted the old way: no lane declared, no
  repositories recorded, no version lock, while `vivac init --dry-run` showed
  a plan doing all three. There is one way to plant now, and it asks before
  writing: with no terminal and no `--yes`, `init` exits `1`, writes nothing,
  and names `--dry-run` and `--yes`. A script that ran `vivac init` alone
  needs `--yes`.
- **A tree planted by a bare `vivac init` in `0.13.0` or earlier has no lane
  and no lock**, so the refusal that stops a second clone of the same product
  from planting a tree of its own never sees it. Run `vivac init` once in the
  tree's own folder: the plan shows the lane and the lock it adds, and nothing
  else. From then on the tree needs vivac `0.12` or newer to read it, as every
  tree with lanes already does.
- **Under a real tree, a folder whose `.vivac/` holds neither a tree nor a
  lane can now be planted with `vivac init`.** Every other command refuses
  there and names `vivac init` as one way out, which `init` itself used to
  refuse. The plan says a tree sits above before anything is written.

### Fixed

- **A session's first write in a new worktree is no longer refused by its
  own hook.** Joining a lane created `.vivac/` first and wrote the lane
  file after, and since `0.13.0` a `.vivac/` holding neither a tree nor a
  lane under a real tree is refused. The opening hook and the first `push`
  joining the same worktree could meet in that gap. A new `.vivac/`, for a
  lane or a tree, now appears with everything in it at once.
- *(store)* make a new .vivac/ appear whole, never empty first
- *(init)* plant through one path, with flags or without

### Documentation

- *(changelog)* say what bare init does now, and what old trees need

## [0.13.0](https://github.com/JAAvila-Of/vivac/compare/v0.12.5...v0.13.0) - 2026-09-23

This is what `0.12.6` shipped, under the number it should have had. It
breaks a public surface, and in `0.x` that is a minor, never a patch.

### Upgrading

- **`vivac setup <harness>` no longer plants the tree, and refuses where
  there is none.** Which tree a folder belongs to has the same answer
  wherever an agent is opened, so it was never a harness's question. Run
  `vivac init` first, then `vivac setup claude-code` or `vivac setup codex`.
  With no tree resolvable from the folder, setup exits `4` — the code every
  other command already gives that condition. With a tree above it but the
  folder not yet one of its lanes, it exits `1` and says why: work written
  from there would be recorded as the tree's own folder rather than as that
  one. A script that ran setup alone in a fresh folder now needs the two.
- **`--join`, `--new-tree`, `--name` and `--lane-name` are `vivac init`'s.**
  Passing one to setup names the command to run instead of failing as an
  unknown flag, the same way `vivac hooks` still points at where it went.
  `vivac setup claude-code --undo` keeps removing the harness's own three
  pieces and nothing else; what `init` wrote, `vivac init --undo` takes
  back.
- **A `.vivac/` that holds neither a tree nor a lane no longer answers as a
  project root.** Undoing a join used to leave one behind holding only its
  `.gitignore`, and from then on that folder read as its own project: the
  brief named the folder instead of the tree above it, and the next setup
  said the tree was already there. The walk up now stops at a tree or a
  lane, never at a directory that merely has the name. Where such a folder
  sits under a real tree, every command refuses and names the two ways out,
  because which of the two it belongs to cannot be read off the disk. Undo
  no longer creates them either.

### Added

- **`vivac setup codex` merges, runs twice without writing, and takes itself
  back**, the same way the Claude Code side does. The server goes into
  `.codex/config.toml` between two marker comments; a block with one marker
  and not the other is left alone and named, in both directions.
- *(setup)* setup writes what a harness reads, and nothing of the tree
- *(init)* let init answer which tree this folder belongs to
- *(setup)* let setup codex take back exactly what it wrote
- *(setup)* let setup codex merge with what the project already has
- *(setup)* give the Codex side the join its own refusal proposes
- *(setup)* give setup codex the tree it was leaving out

### Fixed

- **The setup plan has a width.** Its second column wraps at the same 76
  columns the rest of this binary's prose already used, so a lane or product
  name of any length no longer runs a line past the edge of an 80-column
  terminal and lands under the labels.
- **A join that failed halfway no longer leaves a folder claiming a lane the
  target never received.** The lane file was written before the declaration
  reached the target's log, and the rollback that should have covered it was
  only ever exercised against the other half of that write.
- **`vivac setup <harness> --undo` without a terminal exits non-zero and
  removes nothing**, instead of printing a question nobody can answer and
  exiting `0`.
- *(render)* count what goes in front of a clip, and cut on a word
- a .vivac/ that is neither a tree nor a lane is not a map
- *(setup)* give the plan's second column a width

### Documentation

- write the upgrading note this branch owes, and finish the sweep
- say that leaving a project ready is two commands now
- stop writing the flags as if one harness owned them
- stop saying Codex only works on a clean project
- say what a real Codex session actually showed

### Internal

- *(session)* require the closing hook to print nothing at all

## [0.12.6](https://github.com/JAAvila-Of/vivac/compare/v0.12.5...v0.12.6) - 2026-09-23 [YANKED]

Yanked the day it went out: it broke a public surface under a patch
number. Its contents are `0.13.0`'s, which is where the notes are.

## [0.12.5](https://github.com/JAAvila-Of/vivac/compare/v0.12.4...v0.12.5) - 2026-09-22

### Upgrading

- **`reconcile` now answers for the product, not for the folder you are
  standing in.** When another lane of the same product has changes nothing
  in the tree claims, they come out under a new `IN OTHER LANES OF THIS
  PRODUCT` section, each lane measured against its own last stop. A lane
  whose folder cannot be read is named and skipped. `--since` still measures
  this lane alone, and now says so. `--json` entries gain a `lane` field
  beside the ones already there, so anything counting the old output will
  count more.
- **A relocated product reports one more lane than it did.** `relocate` now
  gives the destination -- the folder that ends up holding the tree -- a
  lane of its own. It had none, and fell through to the founding lane that
  had just been handed to the folder it moved from, so writing from there
  was recorded against a lane living somewhere else. `vivac stack --lanes`
  shows the extra one, and nothing has to be run for it.
- **`vivac setup claude-code --undo` now removes `.vivac/lane`**, when that
  lane has never written anything to the tree. Until now it left the file
  behind, and while the file is there the folder cannot receive the tree at
  all: `relocate` refuses it for holding a lane. A lane that has written is
  left exactly as it was, and `--undo` says why.
- **Four of `setup`'s messages changed wording.** The refusal with no
  terminal repeats the flags you typed, a repository that is the folder
  itself is named rather than printed as a bare dot, planting beside a
  product the registry cannot recognise says so, and joining a folder to an
  existing tree points at migrating what that folder knows. Anything
  matching those messages by exact text will stop matching.
- **The measurements moved out of the README** into `docs/PERFORMANCE.md`,
  and the versioning rule into `docs/VERSIONING.md`. A link to
  `README.md#status` no longer resolves.

### Documentation

- *(setup)* name no memory product in the skill setup writes
- make the README a front page, and its examples real captures

### Fixed

- *(lanes)* one folder, one lane, and a way back out of a join
- *(reconcile)* answer for the product, not for the folder you stand in
- *(setup)* four messages that left out what the reader needed next

## [0.12.4](https://github.com/JAAvila-Of/vivac/compare/v0.12.3...v0.12.4) - 2026-09-21

### Fixed

- *(web)* name a project by the name it was given, not by its folder

## [0.12.3](https://github.com/JAAvila-Of/vivac/compare/v0.12.2...v0.12.3) - 2026-09-20

### Added

- *(setup)* --join finishes the setup, and --name names the product

### Fixed

- *(stack)* --lanes lists every lane, not only the ones with a stack

## [0.12.2](https://github.com/JAAvila-Of/vivac/compare/v0.12.1...v0.12.2) - 2026-09-20

### Added

- *(setup)* leave a project ready for Codex, on a clean project
- *(why)* open a safe stop by its own alias

### Fixed

- *(brief)* stop hiding what the last session was about to do

## [0.12.1](https://github.com/JAAvila-Of/vivac/compare/v0.12.0...v0.12.1) - 2026-09-19

### Upgrading

- **If you once registered the server by hand, `setup` may have added one
  Claude Code does not use.** Before `setup` existed the README said to run
  `claude mcp add vivac -- vivac mcp`, which puts the server in Claude Code's
  local scope, and that registration wins over the `.mcp.json` `setup`
  writes. Nothing stops working. To keep only the project's, run
  `claude mcp remove vivac -s local` from that folder. From this version on,
  `setup` says so itself when a tree that was already there gets the server
  for the first time.
- **`find` ignores accents now, as it already ignored case**, in the CLI,
  over MCP and in the map page's find box. A search returns what it did
  before plus every node spelled the other way, so anything that counts hits
  will count more.
- **`vivacs --json` and `reconcile --json` gain `anchors`**, beside `anchor`
  and always present. In a folder that is not itself a repository `anchor`
  stays null, and `anchors` is where its repositories are.
- **Building from source pulls two more crates**, `unicode-normalization` and
  `tinyvec`. The release binaries need none.
- **On Windows, close every session and any `vivac web` before installing.**
  A running `vivac mcp` or `vivac web` holds the executable open, and
  `cargo install` fails with `os error 5` until it is closed.

### Documentation

- *(readme)* say that find ignores case and accents

### Fixed

- *(find)* stop telling accented and unaccented spellings apart
- *(setup)* warn that a hand-made registration can shadow the server
- *(json)* carry every repository a stop is anchored to

### Performance

- *(find)* lower case runs of ASCII whole and fold only the rest

## [0.12.0](https://github.com/JAAvila-Of/vivac/compare/v0.11.3...v0.12.0) - 2026-09-19

### Upgrading

- **Nothing in a tree changes until `setup` runs in it.** Until then this
  version writes what 0.11 wrote, and 0.11 keeps reading it. The one
  exception is a linked worktree: the first time anything writes from one,
  it declares itself a lane of the tree it belongs to.
- **The brief's header names the lane, and takes that name from the folder.**
  A tree with one lane prints the same bytes it always did. Anywhere else the
  name arrives when `setup` runs and is recalculated every time it runs
  again, so renaming a folder shows up at the next `setup` and never
  repoints anything on its own.
- **Once a tree holds lanes it needs 0.12 or newer.** `setup`, `relocate`,
  or a linked worktree writing for the first time rewrites the config's
  `version` field to `this tree holds lanes, and this vivac is too old to
  read them: update vivac`. Versions 0.7 to 0.11 refuse a version they do
  not know and print that sentence.
- **Three events are new**, and none of them appears before the tree holds
  lanes: `lane.declared`, `lane.claimed` and `where.changed`. The last one
  is a photograph of the branches this lane's repositories were on, written
  only when they differ from the last one this lane wrote.
- **Every tree gains `.vivac/lock`, and `setup` also writes
  `.vivac/.gitignore`.** If `.vivac` was committed to git, `vivac check`
  points at the missing `.gitignore` and gives the command that takes it
  back out.
- **The first read of each tree after upgrading is slower, once.** The
  derived index changed shape, so this version declines the one it finds,
  folds the log, and writes a new one. Nothing to run: the next read is back
  to normal. An older vivac does the same in reverse, and also keeps working.
- **The brief no longer says how many files changed since your last stop.**
  It was costing two git processes on every read to print one line. Ask for
  it when you want it: `vivac changes`, or `vivac reconcile` to see it
  against the tree.
- **`vivac setup claude-code --join` with nothing after it used to plant a
  tree** instead of joining one, silently. It now refuses and says what it
  needs. If you ran it that way, look for a `.vivac/` you did not mean to
  create.
- **Building from source now needs Rust 1.89**, up from 1.75. `cargo install
  vivac` compiles, so this is your toolchain, not your project's. The
  release binaries need none.
- **On Windows, close every session and any `vivac web` before installing.**
  A running `vivac mcp` or `vivac web` holds the executable open, and
  `cargo install` fails with `os error 5` until it is closed.

### Added

- *(model)* give each node the lane and seq it was born with
- *(web)* say when the map's here control is not where you are
- *(web)* show whose focus is on screen once lanes are several
- *(stack)* list every lane's own thread
- *(brief)* say when another folder of this product wrote
- *(model)* record when each lane last wrote
- *(setup)* name the founding lane after its own folder
- *(brief)* say when the branch moved and where that work stopped
- *(why)* say which lane and branch a node was born in
- *(anchor)* anchor a stop to every repository of the lane
- *(store)* write where the lane is when it moves
- *(store)* record where a lane's repositories are
- *(anchor)* read a repository's branch, sha and rebase state
- *(brief)* open with the warning when this tree is a copy
- *(setup)* join a folder to a tree that lives somewhere else
- *(setup)* refuse to plant a second map of a product
- *(relocate)* move a tree and leave its folder as a lane
- *(registry)* record a project's repos and lanes, and notice copies
- *(ops)* let a linked worktree join the tree when it first writes
- *(setup)* make a folder a lane of the tree above it
- *(model)* [**breaking**] give every lane its own stack, focus and stop counters
- *(event)* let a lane declare itself and sign what it writes
- *(store)* resolve the working folder's lane, not just its tree
- *(lane)* name the lane a working folder belongs to

### Changed

- *(brief)* sort the candidates by key instead of by comparator
- *(check)* keep the copy heading where its sentence lives
- *(store)* make the write lock an argument of every append
- [**breaking**] raise the MSRV to 1.89 for the standard file lock

### Documentation

- *(readme)* publish what was measured, and on which machine
- *(help)* say which folder relocate is run from
- *(readme)* point at what setup already says, and count 0.11 and 0.12
- *(lanes)* the public guide, and the README section pointing at it
- *(src)* retire two comments that describe a past that ended
- *(cli)* name --lanes in the help that lists stack
- cite the tree's node, not a review round that no longer exists
- *(setup)* say what joining a folder actually changed
- the last Spanish in the crate was a quoted section title
- *(test)* two comments in relocate's tests were still in Spanish
- three more comments that were still in Spanish
- write the crate's comments in English, as the rule says

### Fixed

- *(args)* --lanes takes no value, and now says so
- *(args)* refuse --join with no value, and make --new-tree a switch
- *(anchor)* see a linked worktree past a submodule's own .git
- *(setup)* preview and apply a stale worktree's redeclaration always
- *(setup)* declare a worktree's lane with the root commit it shares
- *(setup)* name every tree below a join, and guard the route shown
- *(setup)* answer the door the reader knocked on
- *(check)* name a repeated seq and a tail that swallowed a write
- *(brief)* count one other lane in words, not as 1 lanes
- *(store)* resolve a redeclared lane's repositories from its own folder
- *(anchor)* resolve a linked worktree's branch through commondir
- *(setup)* stop a repeated --join from orphaning the folder's lane
- *(t594)* refuse relocate from a copy and close the join bypass
- *(registry)* trigger the copy warning by having written, not the verb
- *(setup)* close the doors joining a folder was walking around
- *(relocate)* refuse foreign lanes, wall off the real registry
- *(relocate)* a concurrent reader can hit the empty-tree bug too
- *(relocate)* never let a failed move look like an empty tree
- *(relocate)* roll back copy failures, refuse moving onto self
- *(ops)* refuse the folder with no lane, not the one named main
- *(registry)* stop copy_of from naming itself, wrap all five forms
- *(registry)* finish closing f612, and warn about every copy
- *(registry)* close f612 in copy detection, and warn both folders
- *(ops)* fall back to canonicalize when repo_at's paths disagree
- *(lane)* ask the log, not config, whether a tree has lanes
- *(lane)* move signing lane off store, gate auto-join on setup
- *(setup)* mint a lane instead of redeclaring a claimed main
- *(ops)* join a worktree on write, not on taking the lock
- *(setup)* make the closing message say what a run wrote
- *(store)* let a lane retry its worktree's main copy on its own
- *(setup)* register the tree a lane joins and stop misreporting it
- *(ops)* keep the working folder's lane when the tree is rebuilt
- *(registry)* write the project registry atomically under a lock
- *(session)* take the lock only when a stop has something to write
- *(mcp)* hold the write lock around every server write
- *(store)* serialize writers with an OS lock
- *(setup)* keep .vivac out of version control
- *(render)* say when a number names more than one node

### Internal

- lint the release profile, which nothing here ever built
- pin a 0.11.3 log and check git status ignores .vivac
- *(setup)* cover the withheld-route branch, plural and mixed
- *(tests)* drop the `todo` exception now that nothing reaches it
- the other lanes, end to end
- *(lanes)* fail loudly if the brief footer loses its token count
- the branch that moved, end to end
- *(t594)* stop the suite from leaking its own temp files
- *(t594)* fix nine review-broken tests that stayed green
- the IQuorum scenario, end to end
- *(check)* cover the fifth copy-notice form, all names withheld

### Performance

- *(why)* read the index again instead of folding the whole log
- *(brief)* stop diffing against git on the read path
- *(mcp)* apply only the tail another process appended

## [0.11.3](https://github.com/JAAvila-Of/vivac/compare/v0.11.2...v0.11.3) - 2026-09-15

### Upgrading

- **Run setup again to update the migration skill.** In each folder where
  you ran `vivac setup claude-code`, run it again: it replaces the
  vivac-migrate skill an earlier release wrote, if nobody changed it since,
  and touches nothing else in the project.
- **If you stopped a migration before it finished, look for an export it
  left.** Until this release the skill deleted an export of another memory
  tool only at its last check, so one made by a migration stopped earlier
  may still be in the temporary folder the agent wrote it to. It holds
  every memory that tool has, for every project: delete it.
- **No event changed.** 0.11.2 reads a log 0.11.3 wrote.
- **On Windows, close every session and any `vivac web` before installing.**
  A running `vivac mcp` or `vivac web` holds the executable open, and
  `cargo install` fails with `os error 5` until it is closed.

### Fixed

- *(setup)* delete an export before asking the person anything

## [0.11.2](https://github.com/JAAvila-Of/vivac/compare/v0.11.1...v0.11.2) - 2026-09-15

### Upgrading

- **Run setup again to update the migration skill.** In each folder where
  you ran `vivac setup claude-code`, run it again: it replaces the
  vivac-migrate skill an earlier release wrote, if nobody changed it since,
  and touches nothing else in the project.
- **If setup told you on an upgrade that nothing had been brought in, it was
  wrong.** Until this release its last message was the one for a first
  setup, whatever it had written. Setup never changes a tree that is already
  there, so everything a migration wrote is still in it.
- **No event changed.** 0.11.1 reads a log 0.11.2 wrote.
- **On Windows, close every session and any `vivac web` before installing.**
  A running `vivac mcp` or `vivac web` holds the executable open, and
  `cargo install` fails with `os error 5` until it is closed.

### Fixed

- *(store)* apply in memory the events exactly as they were written
- *(setup)* end with what this run changed, not the first-run text
- *(setup)* freeze a harness memory that has no index yet
- *(setup)* ask before searching services outside the project

## [0.11.1](https://github.com/JAAvila-Of/vivac/compare/v0.11.0...v0.11.1) - 2026-09-15

### Upgrading

- **Run setup again to update the migration skill.** In each folder where
  you ran `vivac setup claude-code`, run it again: it replaces the
  vivac-migrate skill an earlier release wrote, if nobody changed it since,
  and touches nothing else in the project.
- **If you migrated with 0.11.0 and open Claude Code in more than one
  folder of the project**, the other maps were retired only in the folder
  that session ran in. In each other folder, run `vivac setup claude-code`,
  and turn the memory plugin off there too, with
  `"enabledPlugins": { "<plugin>": false }` in that folder's
  `.claude/settings.json`.
- **No event changed.** 0.11.0 reads a log 0.11.1 wrote.
- **On Windows, close every session and any `vivac web` before installing.**
  A running `vivac mcp` or `vivac web` holds the executable open, and
  `cargo install` fails with `os error 5` until it is closed.

### Documentation

- *(readme)* say other memory systems keep talking until retired

### Fixed

- *(setup)* keep lines of code out of migrated nodes
- *(setup)* retire other maps in every folder Claude Code opens in
- *(setup)* say other memory systems keep talking until retired

## [0.11.0](https://github.com/JAAvila-Of/vivac/compare/v0.10.0...v0.11.0) - 2026-09-14

### Upgrading

- **setup writes Claude Code's files in the folder you run it in.** Claude
  Code reads `.claude/settings.json` and `.mcp.json` only from the folder a
  session is opened in. Under 0.10.0, setup run from a subfolder of a tree
  wrote them next to the tree instead, where that session never read them.
  If you open Claude Code in a folder that does not hold `.vivac/`, run
  setup there: it finds the tree above and writes only that folder's files.
- **setup refuses your home folder.** There, `.claude/` is Claude Code's
  configuration for every project. If 0.10.0's setup ran there,
  `vivac setup claude-code --undo` in your home folder takes out the hooks,
  the server and the skill it wrote. If it also planted a tree, the `config`
  and `events` files it added to the `.vivac` folder in your home can be
  deleted.
- **Running setup again updates the migration skill.** A copy an earlier
  release wrote, and nobody changed since, is replaced by the new one, and
  nothing else in the project is touched.
- **A migration takes one prompt.** Ask the agent to use the vivac-migrate
  skill to bring everything the project knows into vivac. It finds the
  sources itself, shows a plan before writing anything, checks what it
  wrote, and offers to retire the other maps one at a time, only if you say
  yes.
- **No event changed.** 0.10.0 reads a log 0.11.0 wrote.
- **On Windows, close every session and any `vivac web` before installing.**
  A running `vivac mcp` or `vivac web` holds the executable open, and
  `cargo install` fails with `os error 5` until it is closed.

### Added

- *(setup)* end with the prompt that starts the migration
- *(setup)* rewrite vivac-migrate so a migration needs no reviewer
- *(setup)* [**breaking**] write Claude Code's files where it is opened

### Documentation

- *(readme)* run setup where Claude Code opens, migrate in one prompt

## [0.10.0](https://github.com/JAAvila-Of/vivac/compare/v0.9.0...v0.10.0) - 2026-09-14

### Upgrading

- **`vivac hooks` is gone.** `vivac setup claude-code` writes the two hooks,
  the MCP server and the migration skill into the project, after showing
  them and asking. `vivac hooks` now says where to go instead and exits 2.
- **`vivac session start --hook` prints the brief as plain text.** Claude
  Code puts a `SessionStart` hook's plain output into the context just as it
  put the JSON envelope, so hooks set up under 0.9.0 keep working as they
  are. Anything else that parsed the envelope has to read plain text.
- **A project whose hooks were pasted by hand needs nothing.** Running setup
  there finds them in place, even when they spell the path to `vivac`
  differently, and adds only what is missing from the project's files.
- **`vivac init` on an existing tree no longer rewrites its config.** Under
  0.9.0 a second `init` gave the tree a new project id, a new actor for
  every event after it, and dropped the lock 0.8.0 puts on a tree that holds
  pillars or rules. If that happened to a governed tree, delete
  `.vivac/config`: the next command writes it back, locked.
- **A flag that takes no value no longer takes the word after it.**
  `push --blocks "title"` keeps its title, and `--blocks=x` is refused
  instead of stored in silence.
- **No event changed.** 0.9.0 reads a log 0.10.0 wrote.
- **On Windows, close every session and any `vivac web` before installing.**
  A running `vivac mcp` or `vivac web` holds the executable open, and
  `cargo install` fails with `os error 5` until it is closed.

### Added

- *(setup)* [**breaking**] write Claude Code's configuration instead of printing it
- *(session)* [**breaking**] hand the brief to the hook as plain text

### Documentation

- *(readme)* document setup and how to migrate into vivac

### Fixed

- *(init)* leave an existing tree's config alone
- *(args)* stop a flag without a value from eating the next word

## [0.9.0](https://github.com/JAAvila-Of/vivac/compare/v0.8.0...v0.9.0) - 2026-09-14

### Upgrading

- **`done` and `park` step off the stack only from its top.** Closing or
  parking a node below the focus now leaves the stack as it was, and the
  brief marks that node on the spine with its state. Under 0.8.0 the node
  left the stack and the rest stayed, which is how the depth advice came to
  name the wrong goal. `pop` steps off the top as before.
- **`pop` on a focus that is no longer open leaves it as it was.** It steps
  off the stack without writing a new state. 0.8.0 closed it again, which
  turned a superseded decision into a done one and replaced its outcome.
- **`restore` keeps the closed nodes below the deepest open one**, and lists
  them as `still on the path` apart from what left the stack.
- **The brief reads differently, and it was never meant to be parsed.** A
  node on the spine that is no longer open carries its state in brackets.
  DO NOT TOUCH NOW lists every parked node in the project, wherever the
  focus is. A decision hanging off any root counts as project-wide, as an
  invariant already did, and the governing section of `vivac web` follows
  because it reads through the same function. With nothing on the stack the
  brief keeps the invariants, standing decisions, parked nodes and last
  stop, and names a real node to pick up.
- **Every JSON change is an addition.** Over MCP, a push carries
  `left_stack` and `back_to`, and the `closed` object of a pop or a done
  carries `already`. Nothing was renamed or removed.
- **No event changed.** 0.8.0 reads a log 0.9.0 wrote, and both fold it
  into the same stack.
- **Restart every session once you have upgraded.** An MCP server still
  running 0.8.0 keeps unstacking the old way on the same log. On Windows it
  also holds the executable open, and `cargo install` fails with
  `os error 5` until the session that started it is closed.

### Added

- open a node at the root while the stack is on

### Documentation

- *(readme)* describe the stack as a path, --root and the empty brief

### Fixed

- *(brief)* carry project governance from any root and with no focus
- *(stack)* [**breaking**] keep a closed node on the path, and say so on the spine

## [0.8.0](https://github.com/JAAvila-Of/vivac/compare/v0.7.0...v0.8.0) - 2026-09-13

### Upgrading

- **Once `vivac declare` has run on a tree, 0.7.0 no longer reads it.** A
  late declaration is an event 0.7.0 does not know, so it stops on that tree
  with exit code 5, names the line, and writes nothing. A declaration made
  when the decision was written is not an event of its own: 0.7.0 reads that
  decision without showing what it declared, and a decision 0.7.0 writes
  carries no list at all.
- **Restart every session once you have upgraded.** An MCP server from 0.7.0
  that is still running stops the same way as soon as a late declaration
  lands in the log. On Windows it also holds the executable open, and
  `cargo install` fails with `os error 5` until the session that started it
  is closed.
- **`check` can exit with 1 on a tree that passed before.** It now reports
  every open decision written while a pillar or rule was open that declares
  nothing it was judged against. A decision written by an older release, or
  in a tree with nothing to judge against, carries no list and is never
  reported, so upgrading alone turns nothing red: only decisions written
  from now on count, and `vivac declare` clears one.
- **Every JSON change is an addition.** In `why --json` and `vivac_why`, a
  decision written while a pillar or rule was open, or declared later,
  carries `against`, and every entry in it carries `state`. On `path`, a
  rule step carries `arms`, and under `--full` a decision step carries
  `against` too. Every write over MCP carries `text`. Nothing was renamed or
  removed.

### Added

- *(mcp)* return what the CLI prints alongside every write
- [**breaking**] declare which pillars and rules a decision was judged against

### Fixed

- *(why)* carry a rule's arms on each step of the path

## [0.7.0](https://github.com/JAAvila-Of/vivac/compare/v0.6.11...v0.7.0) - 2026-09-13

### Upgrading

- **A tree's first pillar or rule locks it against older releases.** That
  write turns `version` in `.vivac/config` from `1` into the sentence *this
  tree holds pillars and rules, and this vivac is too old to read them:
  update vivac*. Every release up to 0.6.11 then stops on that tree instead
  of misreading it: it prints the sentence inside an input/output error,
  exits with code 5, and leaves the log byte for byte as it was. A tree with
  no pillar and no rule keeps its config, and older releases go on reading
  it.
- **Restart every session once you have upgraded.** An MCP server from an
  older release that is still running reads the config again as soon as the
  log moves, so once the tree is locked it answers with that error. On
  Windows it also holds the executable open, and `cargo install` fails with
  `os error 5` until the session that started it is closed.
- **A tree written by a newer release is refused, never skimmed.** A config
  `version` this release does not know, or a log line naming an event type or
  a node kind it does not know, stops the command with exit code 5, names the
  line, and writes nothing. Until now such a line was skipped in silence, and
  the next write could reuse its number.
- **`why --json` and `vivac_why` changed shape**
  ([#105](https://github.com/JAAvila-Of/vivac/pull/105)). `node` stays whole.
  `path` holds the ancestors only and no longer ends with the node.
  `in_parallel`, `born_here`, `standing` and `open_then` are handles, and
  `blockers` lists what keeps every open step of the path from closing, as
  the prose does. Anything that reads those fields needs updating.

### Added

- add pillars and rules, and read them back with vivac rules
- *(why)* [**breaking**] return handles for everything but the node asked about ([#105](https://github.com/JAAvila-Of/vivac/pull/105))

### Fixed

- *(store)* make older releases refuse a tree that holds pillars
- *(store)* refuse a log that holds events from a newer vivac
- *(open)* stop listing constraints as open fronts
- *(help)* document exit code 5, returned by every i/o error ([#104](https://github.com/JAAvila-Of/vivac/pull/104))

## [0.6.11](https://github.com/JAAvila-Of/vivac/compare/v0.6.10...v0.6.11) - 2026-09-10

### Documentation

- *(readme)* install without a Rust toolchain ([#99](https://github.com/JAAvila-Of/vivac/pull/99))

### Fixed

- *(web)* make the map's where-am-i button work again

## [0.6.10](https://github.com/JAAvila-Of/vivac/compare/v0.6.9...v0.6.10) - 2026-09-09

### Fixed

- *(release)* tell gh which repository, since publish has no checkout ([#97](https://github.com/JAAvila-Of/vivac/pull/97))

## [0.6.9](https://github.com/JAAvila-Of/vivac/compare/v0.6.8...v0.6.9) - 2026-09-09

### Fixed

- *(release)* take the tag from the JSON, and fail when it is missing ([#95](https://github.com/JAAvila-Of/vivac/pull/95))

## [0.6.8](https://github.com/JAAvila-Of/vivac/compare/v0.6.7...v0.6.8) - 2026-09-09

### Internal

- *(release)* build the binaries in the run that made the release

## [0.6.7](https://github.com/JAAvila-Of/vivac/compare/v0.6.6...v0.6.7) - 2026-09-09

### Added

- *(web)* draw the tree as a map you can read a node from ([#92](https://github.com/JAAvila-Of/vivac/pull/92))
- *(web)* flatten the global graph, and draw what blocks ([#90](https://github.com/JAAvila-Of/vivac/pull/90))

### Fixed

- *(model)* keep every note, not only the last one written ([#91](https://github.com/JAAvila-Of/vivac/pull/91))

### Internal

- *(web)* take the port from the server, not from a guess ([#88](https://github.com/JAAvila-Of/vivac/pull/88))

## [0.6.6](https://github.com/JAAvila-Of/vivac/compare/v0.6.5...v0.6.6) - 2026-09-09

### Added

- *(open)* rank fronts by what is waiting, and cap the list at ten ([#87](https://github.com/JAAvila-Of/vivac/pull/87))

### Documentation

- *(pillars)* count the read ceiling in open fronts, not in nodes ([#86](https://github.com/JAAvila-Of/vivac/pull/86))
- *(readme)* say what it does now, and what the numbers were hiding ([#85](https://github.com/JAAvila-Of/vivac/pull/85))
- *(readme)* the web serves four pages, and opens from anywhere ([#82](https://github.com/JAAvila-Of/vivac/pull/82))

### Internal

- *(readme)* fail when the set of web pages changes ([#84](https://github.com/JAAvila-Of/vivac/pull/84))

## [0.6.5](https://github.com/JAAvila-Of/vivac/compare/v0.6.4...v0.6.5) - 2026-09-08

### Added

- *(web)* open from any directory and say which project stopped ([#80](https://github.com/JAAvila-Of/vivac/pull/80))

## [0.6.4](https://github.com/JAAvila-Of/vivac/compare/v0.6.3...v0.6.4) - 2026-09-08

### Documentation

- three places still said find sorts newest first ([#78](https://github.com/JAAvila-Of/vivac/pull/78))

## [0.6.3](https://github.com/JAAvila-Of/vivac/compare/v0.6.2...v0.6.3) - 2026-09-08

### Added

- *(find)* rank hits by field and subtree instead of recency ([#77](https://github.com/JAAvila-Of/vivac/pull/77))
- *(check)* --gates reports trees that nobody opens ([#75](https://github.com/JAAvila-Of/vivac/pull/75))

## [0.6.2](https://github.com/JAAvila-Of/vivac/compare/v0.6.1...v0.6.2) - 2026-09-08

### Documentation

- *(model)* stop claiming the stack bottom is always a goal ([#74](https://github.com/JAAvila-Of/vivac/pull/74))

### Fixed

- *(push)* name the goal this stack came from, not the first root ([#70](https://github.com/JAAvila-Of/vivac/pull/70))

### Internal

- *(release)* group the invisible types instead of dropping them ([#73](https://github.com/JAAvila-Of/vivac/pull/73))
- *(release)* group the changelog by type instead of one Other bin ([#71](https://github.com/JAAvila-Of/vivac/pull/71))

## [0.6.1](https://github.com/JAAvila-Of/vivac/compare/v0.6.0...v0.6.1) - 2026-09-08

### Other

- *(why)* clip ancestor bodies so a read stops dragging the whole spine ([#68](https://github.com/JAAvila-Of/vivac/pull/68))

## [0.6.0](https://github.com/JAAvila-Of/vivac/compare/v0.5.1...v0.6.0) - 2026-09-07

### Added

- *(mcp)* the tools can reach a node in another tree ([#63](https://github.com/JAAvila-Of/vivac/pull/63))
- *(find)* --everywhere searches the trees the machine knows ([#61](https://github.com/JAAvila-Of/vivac/pull/61))
- *(registry)* a project enters by being used, keyed by its first event ([#58](https://github.com/JAAvila-Of/vivac/pull/58))
- *(open)* [**breaking**] the fronts come back as fronts, not as whole nodes ([#53](https://github.com/JAAvila-Of/vivac/pull/53))
- *(mcp)* the seven capture seams become tools an agent can call ([#47](https://github.com/JAAvila-Of/vivac/pull/47))
- *(store)* the tree loads from a derived index, not the whole log ([#43](https://github.com/JAAvila-Of/vivac/pull/43))

### Fixed

- *(registry)* the global store came back from the list as a project ([#66](https://github.com/JAAvila-Of/vivac/pull/66))
- *(registry)* a new project waited a command too long to be registered ([#62](https://github.com/JAAvila-Of/vivac/pull/62))
- *(store)* the global store answered the upward search ([#59](https://github.com/JAAvila-Of/vivac/pull/59))
- *(model)* a blocker only reaches an ancestor through blocking links ([#44](https://github.com/JAAvila-Of/vivac/pull/44))

### Other

- *(position)* two of the three debts had been paid and were still owed ([#67](https://github.com/JAAvila-Of/vivac/pull/67))
- *(readme)* what shipped today, and what was never missing ([#65](https://github.com/JAAvila-Of/vivac/pull/65))
- *(readme)* the read numbers were taken on a tree that cannot cache ([#64](https://github.com/JAAvila-Of/vivac/pull/64))
- *(readme)* the store in your home had no public prose ([#60](https://github.com/JAAvila-Of/vivac/pull/60))
- *(mcp)* three shipped changes left their prose behind ([#57](https://github.com/JAAvila-Of/vivac/pull/57))
- *(hooks)* spell out the matcher instead of relying on its absence ([#56](https://github.com/JAAvila-Of/vivac/pull/56))
- *(readme)* re-measure the whole table rather than patch two cells ([#55](https://github.com/JAAvila-Of/vivac/pull/55))
- *(output)* one owner of stdout instead of a flush per line ([#54](https://github.com/JAAvila-Of/vivac/pull/54))
- *(readme)* the status section outlived the measurements it quoted ([#52](https://github.com/JAAvila-Of/vivac/pull/52))
- *(pillars)* the storage section still promised a rejected migration ([#51](https://github.com/JAAvila-Of/vivac/pull/51))
- *(readme)* the write budget holds now, so say that instead ([#50](https://github.com/JAAvila-Of/vivac/pull/50))
- *(mcp)* a write uses the tree the server already has ([#49](https://github.com/JAAvila-Of/vivac/pull/49))
- *(readme)* the MCP write path does not meet the budget, say so ([#48](https://github.com/JAAvila-Of/vivac/pull/48))
- *(readme)* what happens when a premise turns out false ([#46](https://github.com/JAAvila-Of/vivac/pull/46))
- *(mcp)* say what the server replaces without naming it ([#45](https://github.com/JAAvila-Of/vivac/pull/45))
- *(model)* the tree's edges are numbers, not ULID strings ([#42](https://github.com/JAAvila-Of/vivac/pull/42))
- *(model)* the tree owns its text and the nodes hold spans into it ([#40](https://github.com/JAAvila-Of/vivac/pull/40))

## [0.5.1](https://github.com/JAAvila-Of/vivac/compare/v0.5.0...v0.5.1) - 2026-09-05

### Fixed

- *(ops)* guard the session hook's untrusted fields

### Other

- *(web)* guard the prose in src/web's literals and comments

## [0.5.0](https://github.com/JAAvila-Of/vivac/compare/v0.4.3...v0.5.0) - 2026-09-05

### Added

- [**breaking**] refuse a write that opens a fenced code block

### Other

- the file-contents rule has a mechanism now, and a limit
- *(web)* ban the direct path from src/web to the store
- *(identifiers)* walk src and tests down, not just their top level

## [0.4.3](https://github.com/JAAvila-Of/vivac/compare/v0.4.2...v0.4.3) - 2026-09-05

### Added

- *(web)* draw the whole tree, so its shape can be seen at all ([#32](https://github.com/JAAvila-Of/vivac/pull/32))

## [0.4.2](https://github.com/JAAvila-Of/vivac/compare/v0.4.1...v0.4.2) - 2026-09-04

### Added

- *(web)* the lineage of a node, drawn, at `/p/<id>/why/<node>`. `why`
  narrates the path from the root; this draws it, one step per node, with
  what still governs there and what was open at the time.

### Fixed

- *(web)* **`vivac web` could not be used in a browser at all**, in 0.4.0 and
  0.4.1. The session token was accepted only in an `X-Vivac-Token` header,
  and a browser following a link sends no header the page chose, so every
  page after the boot URL answered 401 and the boot URL itself rendered a
  page that said only that the server was listening. The token now travels
  in a session cookie as well, and the boot URL redirects to the front page.
  The header still works, so `curl` and scripts are unaffected.

## [0.4.1](https://github.com/JAAvila-Of/vivac/compare/v0.4.0...v0.4.1) - 2026-09-04

### Added

- *(decide)* --parent, so a decision can be born where it belongs

### Fixed

- *(help)* decide accepts three flags its help never announced

## [0.4.0](https://github.com/JAAvila-Of/vivac/compare/v0.3.7...v0.4.0) - 2026-09-04

### Added

- *(find)* [**breaking**] hits come back as handles, not whole nodes

  A hit carries `alias`, `kind`, `state`, `title`, `lineage` and `matched`,
  and nothing else. `matched` changed from a list of the field names that
  matched to an object mapping each of those fields to the fragment that
  matched inside it. Gone from a hit, and answered by `why <alias>` instead:
  `id`, `num`, `why`, `blocks`, `parent`, `note`, `outcome`, `refs`,
  `governs`, `opened`, `closed`, `false_close`, `open_below`, `total_below`.
  Both `find --json` and the `vivac_find` MCP tool return this shape: they
  are the same payload and cannot drift apart.

### Other

- *(readme)* say what 0.x promises and what keeps 1.0 away
- *(readme)* warn that a running MCP server blocks updates on Windows
- *(contributing)* say where a security flaw gets reported
- *(release)* the held run is approved, not closed and reopened

## [0.3.7](https://github.com/JAAvila-Of/vivac/compare/v0.3.6...v0.3.7) - 2026-09-03

### Added

- *(why)* what each step of the path looked like at the time

### Other

- *(readme)* hold the README's promises against the binary
- *(readme)* correct three claims the binary contradicts

## [0.3.6](https://github.com/JAAvila-Of/vivac/compare/v0.3.5...v0.3.6) - 2026-09-03

### Added

- *(web)* today, the page that says what moved while you were away
- *(changes)* measure a stretch from the last stop made by hand

### Fixed

- *(triage)* count the depth to a node's goal, not to the root

## [0.3.5](https://github.com/JAAvila-Of/vivac/compare/v0.3.4...v0.3.5) - 2026-09-03

### Added

- *(changes)* what a stretch of work moved, since a stop
- *(web)* the tree in a browser, on this machine and nowhere else

### Fixed

- *(cli)* check the flags before the commands that return early

### Other

- *(pillars)* add UX as the fourth pillar, with a burden of proof
- *(project)* move the long-lived tree out of the mcp server

## [0.3.4](https://github.com/JAAvila-Of/vivac/compare/v0.3.3...v0.3.4) - 2026-09-03

### Other

- state the thesis as one map, not two records side by side ([#12](https://github.com/JAAvila-Of/vivac/pull/12))

## [0.3.3](https://github.com/JAAvila-Of/vivac/compare/v0.3.2...v0.3.3) - 2026-09-02

### Added

- *(mcp)* the reads as tools an agent can call
- *(find)* text search over every field that carries meaning

### Other

- *(ops)* return what happened instead of printing it
- match the changelog to the shape the machine writes
- point the readme at the pipeline that now exists
- derive the version and publish from the commit log
- record the releases up to 0.3.2 in a changelog
- ignore the bytecode the commit guard leaves behind ([#3](https://github.com/JAAvila-Of/vivac/pull/3))
- run the suite, the linters and the guards on every pull request

## [0.3.2](https://github.com/JAAvila-Of/vivac/compare/v0.3.1...v0.3.2) - 2026-09-01

### Added

- *(session)* the automatic stop says what its segment held
- *(session)* record the opening of a session in the log
- *(session)* record what the brief claimed on each opening

### Fixed

- *(cli)* refuse what the parser used to drop in silence
- *(session)* refuse the Spanish spellings of the hooks
- *(session)* an opening is not a change to the tree
- *(cli)* refuse an id that names nothing instead of hitting the focus

### Other

- *(event)* the body no longer carries the aliases it promises
- *(cli)* the help for add announces the flags it takes
- state the project's position, and close contributions for now
- rename the Spanish identifiers d45 left behind
- guard the identifiers, not just what the binary prints
- correct the test count in the status section

## [0.3.1](https://github.com/JAAvila-Of/vivac/compare/v0.3.0...v0.3.1) - 2026-08-31

### Fixed

- *(brief)* a decision that governs the whole project reaches the brief

## [0.3.0](https://github.com/JAAvila-Of/vivac/compare/v0.2.1...v0.3.0) - 2026-08-31

### Added

- *(reconcile)* the diff between the tree and the anchor's history
- [**breaking**] one language. the Spanish compatibility layer is gone

### Fixed

- *(cli)* two messages that were never translated, and derive the word list

### Other

- cargo fmt over the sources
- the last Spanish identifiers, and none of the Spanish that carries weight

## [0.2.1](https://github.com/JAAvila-Of/vivac/compare/v0.2.0...v0.2.1) - 2026-08-31

### Fixed

- *(render)* output that read half-translated after the rename
- *(cli)* block --off printed half a sentence, and add the guard

## [0.2.0](https://github.com/JAAvila-Of/vivac/releases/tag/v0.2.0) - 2026-08-31

### Added

- first cut of the provenance tree in Rust
- close Tier 0 with the brief, the vivacs and the session hooks
- *(brief)* count what stays open under a closed node
- *(abandon)* rescue without reparenting
- *(triage)* the pruning view the brief already named

### Fixed

- reject an unknown option instead of ignoring it
- *(brief)* a standing decision is not an open front
- *(session)* one stop per turn is not a stop

### Other

- add the LICENSE-APACHE the Cargo.toml already promised
- pin line endings to LF with .gitattributes
- update the test count after the two brief fixes
- point the repository at the account that will host it
- install from the registry, not from source
- move every public string and comment to English
- rename every identifier, keeping the old logs readable
- the README and the pillars in English
- mark new_empty as used; mod common compiles once per binary
- the redaction guard advice, which the prose pass missed
