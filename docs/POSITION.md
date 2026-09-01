# Where it sits

None of this is a new problem. Teams have been recording their work for
decades, and coming back cold to something you half-finished predates agentic
sessions by a long way. So the question worth answering is not *"is this
new?"* but **"which question can it answer that the things already on your
disk cannot?"**

It is one question, and it is narrow on purpose:

> **What was this born from?**

What follows describes categories of tool, not products. Every one of these
categories is reasonable, widely used, and better than this at something — the
last section says at what.

## What each category stores

### Logbooks and development journals

They store what happened, in the order it happened. That order is the problem:
**time is the opposite of provenance.** Two entries written the same afternoon
usually belong to unrelated threads, and the entry that explains a decision
ends up far from the entry that acts on it. The measurement in the
[README](../README.md#the-problem) is what that looks like at scale: the path
from the goal to the day's work was six levels deep, inside a tracker ordered
by date.

### Decision records

One file per decision: context, decision, consequences. It is the closest
category to this one, and the gap is precise. A decision record says what was
decided and why. It does not say **which earlier decision raised the
question**, and nothing marks it stale when the premise underneath it stops
holding. A directory of them is a set. Provenance needs a tree.

### Issue and task trackers

They store state — open, closed, assigned — and that closure state is real,
which is more than most of the others manage. What is missing is the edge. The
parent is a link, a label, a field somebody has to remember to fill in, and
**an edge that depends on somebody remembering is an edge that is absent most
of the time.** The hierarchy lives in the schema rather than in the data.

### Version control history

It stores the change and its order with a fidelity nothing else comes close
to, which is why it is the one record nobody has to be talked into keeping.
What a commit cannot store is the **intent that spawned it**. The history will
tell you every line that changed, and not which question you were trying to
answer.

### Session memory for agents

The newest category and the sharpest comparison, because it solves a real
problem and solves it well: an agent starts every session knowing nothing, and
these tools make that stop being true. They keep typed records and hand them
back by search or by recency when a session opens.

They store the **node**. What they do not store is the **edge of birth** — a
record says what was learned, not which piece of work it came out of, so there
is nothing to walk back along. There is no **focus** either: everything
recalled is equally present, and none of it says *you are here*. And with no
**open and closed state**, nothing can be reported as still missing.

The failure that produces is specific, and it is not forgetting. Everything is
written down, retrieval works, and the thing you needed does not come back —
because nothing ever connected it to what you are doing now.

## The three mechanisms

Subtract all of the above and what is left is not a better memory. It is three
structures:

| Mechanism | Question it answers |
|---|---|
| **the edge of birth** | what was this born from? |
| **the focus stack** | where am I, and what must I not touch? |
| **the closure rule** | what is still missing before this can close? |

They are not three features that happened to arrive together. All three rest
on the same thing — a record with **open and closed state** — which is why
they cannot be added to a store that lacks it, and why one more relation type
in such a store does not produce them.

They share a second property, and it is the one that decides whether any of
this survives contact with a real working day: all three are written **without
anybody judging relevance**. The edge is created by `push`, because you were
going to say what you are starting anyway. The stack moves on `pop`, because
you cannot close something without saying how it went. That constraint comes
from the DX pillar, and the measurement behind it is in
[PILLARS.md](PILLARS.md#capture-cannot-cost-more-than-losing-the-thread).

## Where the other categories are better

This is the section that makes the rest worth reading.

- **Recall across everything you have ever done.** A memory store with
  thousands of records and full-text search answers *"have I seen this
  before?"* over a far wider surface, and asks no structure of you in order to
  do it. This answers questions about one tree, and only about work somebody
  opened a node for.
- **Noticing that a record went stale.** Some memory stores attach a review
  date to each record by type and surface what is due. Here that is
  `vivac flag <id> stale`, by hand — which is worse, and known to be worse.
- **Collaboration and process.** Assignment, notification, estimation,
  reporting to people outside the work: that is what trackers are for. None of
  it is here, and none of it is planned.
- **Fidelity about what changed.** That belongs to version control and always
  will. This reads the anchor's history; it does not duplicate it.
- **Explaining a decision to a newcomer in prose.** A decision record is better
  shaped for that. A node's title is one line on purpose.

Running one of these alongside this tool is the expected case, not a
compromise.

## The rule that follows

**It does not compete on memory. It competes on structure.**

Every feature is justified against that sentence. If it is a better way to
remember things, it belongs in a memory store and not here. If it makes the
shape of the work legible, it is in scope. That is a rejection criterion in
the same sense as the [pillars](PILLARS.md), and it exists to be used to say
no.
