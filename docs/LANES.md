# Lanes

A tree records one product. Work on that product happens in folders — one
today, three by Thursday, and rarely the same three next month. A **lane** is
one of those folders, seen from the tree.

That is the whole idea, and the rest of this page is what follows from it.

## Two shapes, and both of them are ordinary

### Several branches in one folder

One checkout, and you move between branches in it all day. The work does not
change folders; the branch under it does.

A tool that tied its record to the branch would fork the record every time you
did, and you would come back to a hotfix branch to find the tree empty — not
because nothing was recorded, but because it was recorded somewhere you were
not standing.

### One product in several folders

The same product, checked out more than once: a clone for the feature, another
for the release you keep having to patch, a third on a machine you only use on
Tuesdays. Same product, same history, different folders.

A tool that tied its record to the folder would give you three trees for one
product, and the question this whole tool exists to answer — *what was this
born from?* — would have three different answers depending on where you were
standing when you asked.

### The one in between

A linked worktree is both at once: one repository, several folders, a branch
checked out in each. It gets the treatment its shape deserves — **a linked
worktree is its own lane**, because it is its own folder, even though it shares
a repository with the lane it was made from.

You do not run anything to make that happen. A worktree becomes a lane the
first time something writes from it, and not a moment earlier: a worktree an
agent created to try something and will delete this afternoon should not leave
a lane behind for having been looked at.

## What a lane is

A lane is a working folder. It is marked by `.vivac/lane`, which holds an
opaque id, the tree it belongs to, and nothing else — no path, no branch, no
repository. Everything else about that lane lives in the tree's log, where
every lane can see it.

What belongs to the **tree** is the work: the nodes, the decisions, the edges
between them. What belongs to a **lane** is where you are in it: the stack, the
focus, the last stop, the session.

So two people in two folders build one shared history and keep two separate
places in it. And two sessions in the *same* folder share a lane, deliberately:
the folder is the unit, not the process.

### A lane is not a git branch

This is the sentence to keep.

The branch is a fact about **each write**, not about the lane. When a node is
written, the tree records which repositories that lane declared and what branch
each was on at that moment. Change branch and the lane does not change: not its
id, not its name, not its stack. The next write simply records a different
branch, and the brief tells you the branch moved.

Two consequences worth spelling out:

- **A lane outlives any branch in it.** Branches are created and deleted; the
  lane is the folder, and it is there until the folder is.
- **A node remembers the branch it was born on**, so `vivac why` can tell you
  that a decision came from a branch that no longer exists. That is the point:
  the branch is gone, the reason is not.

The lane's *name* is not its identity either. The name is the folder's name,
and it is recalculated every time `vivac setup` runs there. Rename the folder
and the tree keeps showing the old name until the next `setup` — the id under
it never moved, so nothing is lost, and nothing is silently re-pointed.

## Setting one up

### Plant a tree

In the folder that holds the product, with no tree above it and none below:

```sh
vivac setup claude-code
```

That plants the tree and makes this folder its first lane. The product takes
this folder's name; pass `--name` to give it another one, which is worth doing
when the folder is a version and not the product — a tree planted in `v2` is a
product called `v2` until you say otherwise.

### Add a folder to a tree that already exists

Run the same command in the new folder. If the tree is above it, the folder
becomes a lane of that tree and there is nothing else to say.

If the tree is somewhere else entirely — another clone, another disk — name it:

```sh
vivac setup claude-code --join "<project>"
```

`--join` takes the project's name or the path to it. Add `--lane-name` to call
the lane something other than the folder's name.

Setup knows when two folders are the same product, because it compares the
repository's root commit against the projects it already knows. When it finds a
match it stops and asks which you meant, rather than guessing — `--join` to
make this folder a lane of that product, `--new-tree` to insist that this one
is genuinely separate.

### Move a tree

```sh
vivac relocate <destination>
```

**Run it in the folder that holds the tree**, not in one of its lanes. It
copies the log, verifies the copy byte for byte before it removes anything, and
leaves the old folder as a lane of the tree in its new home, so every lane
keeps working and nothing has to be repointed by hand.

## What you see

Four surfaces, and each one exists because something happened that you would
otherwise have found out about later.

**The header** names the lane you are in, on every brief.

**`BRANCH MOVED since this lane last wrote`** sits right behind the header when
a repository in this lane is on a different branch than it was on your last
write. It names the repository and both branches, and when it can find where
you left that branch it offers the node to step back into. It is never
truncated — it is a handful of lines by construction, and a brief that dropped
it would be a brief that let you carry on in the wrong place.

**`OTHER LANES since you last wrote here`** lists the lanes that wrote to this
tree since you last did, each with what it is working on. It is the last
section, so a tight budget trims it first — but it is not allowed to disappear
in silence: when it will not fit, it collapses to one line saying how many
lanes wrote, and points at the command below.

```sh
vivac stack --lanes
```

Every lane of this tree, what each is focused on, and when. A lane whose folder
is no longer there is marked rather than hidden.

**`vivac why <id>`** says where each node was born: the lane, the repository
and the branch. When that branch is not the one you are on now, it says so.

## What it does not do

- **It does not merge trees.** One tree per product. If you find yourself with
  two trees for one product, `relocate` and `--join` are the way back to one;
  there is no command that fuses two histories, and there will not be one,
  because a merged provenance that guessed at its own edges would be worse than
  two honest trees.
- **It does not retire a lane.** A lane whose folder is gone is marked in
  `stack --lanes` and dropped from OTHER LANES, and its history stays exactly
  where it was. Nothing is deleted, because what that lane did is still part of
  how the product got here.
- **It does not guess whether anything merged.** It reports that a branch
  changed. It never diffs two branches to decide whether the work on one
  arrived in the other — that is git's question, and a wrong answer to it would
  be silently wrong.
- **It does not sync machines.** A tree is a file on a disk. Two machines with
  two trees for the same product have two trees, and nothing reconciles them
  behind your back.

## Four ways to get this wrong

**Copying `.vivac` into another folder.** A copy carries the tree's identity,
so both folders claim to be the same tree. This is noticed: the brief opens by
saying this tree looks like a copy of another, and says which. Take the warning
seriously — the fix is to delete one and join it properly.

**Committing `.vivac`.** It is this machine's record; a copy of it in every
clone would diverge from every other copy within a day. `vivac setup` writes a
`.gitignore` inside `.vivac` that keeps it out, and `vivac check` tells you
when an older tree is missing it, along with the command that un-commits what
already went in.

**Nesting one tree under another.** Setup refuses, lists the trees it found
below, and tells you how to move them. It refuses rather than picking, because
the two readings — *this is one product* and *these are two* — are both
plausible and only you know which is true. A folder whose name trips the
redaction guard is neither named nor counted — a path printed in an error
message is a path published, and so is how many of them there were.

**Deleting the folder that holds the tree.** The lanes survive, the history
does not. `relocate` exists so that moving a tree is a command rather than a
gamble.
