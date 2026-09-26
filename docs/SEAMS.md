# When each command runs

`vivac` on its own lists the commands, and [Using it](USAGE.md) groups them by
who runs them. Neither can tell you **when**. A list of commands has no place
for the moment in the work each one belongs to, and that moment is what this
page is: the seams of a first session, in the order you meet them, and who
acts at each.

Capture hangs off the **seams** of the work — starting something, settling a
choice, telling you what was found, being told *not now*, finishing — and
never off a judgement that something matters enough to write down. A seam
happens whether or not anybody is thinking about the tree, which is what
keeps writing to it cheaper than losing the thread.

Most of the moments below are the agent's. The page is written for you
anyway: the agent already gets its own version of it, and you are the one
who notices when it skips one.

---

## Once, before anything — you

```sh
vivac init
vivac setup claude-code
```

In the folder you open your agent in. That is everything you run to start;
[the README](../README.md#the-first-five-minutes) walks it, and
[Setting it up](SETUP.md) says what each one writes.

---

## The session opens — nobody

A hook hands the agent the **brief**: where work was left, what governs that
point, what not to touch. It ends with the seams, one line each with the
command for it, and that block is printed by the binary, so it cannot fall
behind the version you have installed. It arrives again after a compaction,
which is when whatever the agent was told earlier has just been thrown away.

You see none of it and nothing is asked of you. `vivac brief` shows you what
it read.

---

## While the work goes on — the agent

| The moment, as you see it in the conversation | What goes in the tree |
|---|---|
| It starts on something, or you ask for something new | a node, opened under the one it continues, after looking whether the tree already holds it |
| A choice gets made, by you or with you | a decision, with what was turned down — when it is made, not at the end |
| It tells you something it found | a finding, one for each thing, as it tells you. One that asks nothing of anyone, a measurement or a lesson, is closed on the spot as a record |
| You say *not now*, *later*, *leave that* | the node, parked with your words; the next brief lists it under **DO NOT TOUCH NOW**, so the next session does not pick it up |
| Something changes where git cannot see it: CI settings, a tracker, a cloud console | a note on the node it belongs to. The tree is the only record that change will ever have |
| The work is done | the node closes with its outcome, and so does the one it returns to, if that settles it too |

The agent does all of it from the command line or over [MCP](SETUP.md#mcp),
and both write the same events.

### Where it slipped, and what changed

Measured over seven real sessions on 0.14.2, across two harnesses, nearly
every seam fired on its own nearly every time. Two things did not hold.
**The finding the agent told you was the one it left out**: in five of the
seven it reported something new and wrote none of it, above all when what it
reported came out of a document it had just read. And in a long session
**the seams faded**, because they reached the agent when the session opened
and not in the turn where the work happened. In one session the agent spent
thirteen minutes on real changes — build definitions edited outside git,
commits in three repositories, a decision turned around — and wrote nothing
until the person asked whether it was recording any of it.

0.15.1 answered both. The seam for findings now names its moment, *as you
tell the person*, and a hook puts one line in front of the agent once it has
worked ten minutes, its own time and not yours, without writing anything
([Setting it up](SETUP.md#what-it-writes-and-where) says how it counts).
Checked in a real session that ran past those ten minutes: the line arrived,
the agent wrote back everything it had let pass, and it went on writing at
its seams afterwards.

### If it slips anyway

No hook reads the conversation. Deciding what in it was worth keeping would be
a judgement, and capture never waits on one. So between two of those lines
the one who notices a skipped seam is you, and a sentence brings it back:

| After | Say |
|---|---|
| it reports something | *Is that in the tree?* |
| you have chosen | *Write that down as a decision.* |
| you put something off | *Not now. Park it.* |
| a long stretch of work | *Are you recording this in vivac?* |

---

## Something turns out false — you

A premise you built on is refuted, and everything on top of it still reads as
valid. Mark it the moment you find out, not when you get round to tidying:

```sh
vivac flag 7 suspect --why "profiled again: the time is in I/O"
```

The node keeps its state, and every brief lists it under **FLAGGED** until
the mark comes off. When you are sure it is wrong, abandoning it and rescuing
what outlived it is [its own section](USAGE.md#when-a-premise-turns-out-false).

Both are yours. The agent will tell you it found the premise false, and that
is a finding; marking and discarding reshape the tree rather than record work,
so they are not among its tools over MCP.

---

## You stop for the day — either of you

Every turn leaves an automatic stop if the tree changed, so nothing depends on
the session closing cleanly. A stop with a name is for the moment you would
otherwise leave yourself a note: the end of the day, a handover, the minute
before something risky.

```sh
vivac save "before touching the adapter" --next "extract the validator"
```

What goes after `--next` is what the next brief says you were about to do.

---

## You come back — you

Days later, a new session, or somebody asking why a thing is the way it is.
That is the moment the reads are for, and the agent's session already opened
with the first of them. The
[third step of the first five minutes](../README.md#the-first-five-minutes)
names the four you will reach for.

---

Every flag, and every command with no moment of its own, is left out on
purpose: `vivac` on its own lists them, and that list is the one that cannot
go stale.
