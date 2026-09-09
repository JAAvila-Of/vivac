# The four pillars

**They govern by definition.** Every design decision is justified against them,
and if it contradicts them it does not get in. They are not aspirations: they
are rejection criteria.

## How they arbitrate between themselves

They will collide. When they do:

| Pillar | Power |
|---|---|
| **Security** | **veto**. It can kill an entire feature, with no negotiation |
| **Performance** | **budget**. It sets a ceiling the feature must respect in order to exist |
| **UX** | **burden of proof**. A reading surface has to say what a person learns from it that they were not going to learn. No answer, no surface |
| **DX** | **judge**. It decides between the options that cleared the other three |

Read in that order they ask four different questions: security asks whether this
may exist at all, performance asks what it is allowed to cost, UX asks whether
anybody learns anything from it, and DX decides what shape it takes. Only the
first kills outright. The other two kill by going unanswered, which is slower and
just as final.

A real collision, and why the order matters: semantic search wants an embedding
model. That is either a network call (slow, and it asks for a key) or a local
model (heavy). Security does not want keys; performance does not want network
on the write path. **Forced conclusion: the embedding never goes on the write
path, and the tool has to work whole without it.** No DX argument can reverse
that.

A second collision, settled on 2026-08-31 while the port was being written: the
model asked for each event's `actor` to be `git config user.email`. That is
personal data. **Security vetoed and the model was corrected**: the actor is an
opaque identifier generated at `init`.

A third, on 2026-09-03, and it is the one that shows the newest pillar doing
work. The web design listed five surfaces. Two of them -- a decisions board and a
list of open fronts -- are things `vivac tree` and `vivac open` already answer
well from a terminal, and neither could say what a person would learn from the
page that the command was not already telling them. **Both were cut, and not for
budget.** The one that survived first answers it in a sentence: the drawn
lineage is the only place in the product that shows the *shape* of the path
rather than its text.

## Correctness over cost

**The right way to do a thing does not depend on what it costs.** Expense is not
an argument for a bypass, a shortcut or a special case. If something is worth
doing it is done properly; if it is not worth doing it is not done. Those are
the only two answers, and "the cheap version, for now" is not a third.

This does not soften the performance budget, it sharpens it. A budget kills a
feature whole; it never licences a degraded one. What the rule forbids is the
outcome where something half exists because doing it right looked expensive.

It is not a licence to build more. Scope is decided elsewhere and ruthlessly:
"do we need this at all?" stays wide open, and dropping a requirement is always
available. What is not available is keeping the requirement and meeting it
badly.

Like the pillars, it is a rejection criterion. "That would take too long", "that
is a lot of work" and "we can fix it later" are not reasons, and a design that
rests on one of them does not get in.

---

## Pillar 1 — DX

The people who use this are developers, and one of them is not a person. It has
to be intuitive at a terminal and scriptable from a program.

*"And visually good: a well-designed interface, not a text dump"* used to be the
rest of this paragraph. It moved out on 2026-09-03 and became Pillar 4, because
sitting here it was an aspiration in a document whose first paragraph says it
holds none.

### Two audiences, and this is the design fork

| Who | What they do | What they need |
|---|---|---|
| **The agent** (executor) | writes: opens nodes, closes them, notes | **CLI**: quiet, scriptable, exit codes, parseable output |
| **The maintainer** (human) | reads: navigates, understands, decides | **the web interface**: the tree, the `why` path drawn, filters, navigation |

**A rule with teeth: everything the agent needs to do has to be doable from the
command line.** If a feature only exists in the interactive interface, an agent
cannot use it and the project loses half its users. The interface is never the
only road to anything, and it has no operations of its own — every one of them
is a command that already exists.

That rule used to name a TUI, and the reasoning is why the TUI is not being
built: maintaining two reading surfaces is exactly what the rule exists to
prevent, because each feature then gets built twice or lives in one of them.
There is one, and it is a page.

### Capture cannot cost more than losing the thread

This is the original thesis and it is still the DX criterion that decides
everything else. If recording a node costs more than a short command, it does
not get recorded, and with no record there is no tree.

Measured on 2026-08-30, and it is the reason this project exists in this shape:
`focus` and `pop` were called **9 times in 170 minutes** because they leaned on
the natural seams of the work (you cannot close a pitch without saying how it
went). In those same 170 minutes, a save operation that asked for a judgement
— "was this relevant?" — was called **0 times**, under a protocol declared
mandatory, because that judgement competes with the work and loses under load.

**Design corollary: operations hang off the seams of the work, never off a
judgement of relevance.**

### Visual rules

- **Meaning is never encoded in colour alone.** `[x]`, `[~]`, `*` and
  `<== FALSE CLOSE` read in black and white. Colour reinforces, it does not
  inform.
- Degrade without breaking: no colour when there is no tty, working over ssh,
  and in Windows Terminal as well as cmd.exe.
- No spinners and no animation on the agent's path.

---

## Pillar 2 — Performance

The server has to be **fast**. It stores and returns the data of an agentic AI,
which means it sits on the critical path of every turn: whatever it takes, the
user pays for by waiting.

### Budgets

**To be measured and corrected, not truths.** The numbers measured against them
are in the `README`.

| Operation | Ceiling | Why |
|---|---|---|
| writing a node (`add`, `done`, `note`) | **p99 < 5 ms** | it sits on the critical path of the agent's turn |
| `why` over 10,000 nodes | **< 50 ms** | it is interactive reading; above that you feel it |
| `brief` / `open` / `tree`, counted in **open fronts** and not in nodes | **< 50 ms** | the same reading; what these three walk is the open part of the tree |
| text search | **< 100 ms** | same |
| semantic search | off the critical path | see the arbitration above |

**The third row used to be half of the second, and it counted the wrong
thing.** The ceiling read *`why` / `tree` over 10,000 nodes*, and what `brief`,
`open` and `tree` cost is not governed by how many nodes a tree holds but by
how many of them are still open. Measured at one size, over the resident
server: `open` answers in 4.4 ms with a hundred and seventy fronts open, and in
27.4 with three and a half thousand — six times the work out of the same ten
thousand nodes. A ceiling stated against a variable that does not move the cost
cannot be enforced, and cannot even be checked: *is `open` over budget?* had no
answer here, and that it had none is the whole defect.

**The variable was already written down, one section below.** Storage names the
two queries that matter as *ancestors of a node* and *open descendants of a
node*. The first is `why`, and it holds flat across both shapes — 2.9 ms either
way — because a lineage is bounded by depth rather than by how much is open.
The second is the other three. The document knew which two shapes exist and set
its ceiling against neither.

**`brief` and `open` were not in this table at all**, which was an omission and
not a decision: the `Why` column says interactive reading, `brief` opens every
session, and a read does not escape the ceiling by never having been typed in
here.

**This row said "missed today" for a few hours on 2026-09-08, and the
retraction belongs here rather than in a changelog.** It was written on the
numbers the `README` was publishing, and those came off a fixture whose
generator exists nowhere any more — so they cannot be re-run, and neither can
the miss. Rebuilt from scratch and measured on a bench that can be run again,
no read misses this ceiling at ten thousand nodes in either shape. What that
buys is not *the ceiling is safe*: it is that the earlier miss was never
checkable, and neither is its absence against it. Writing a miss into the
document that arbitrates, on a number nobody could reproduce, is the same
defect as the ceiling this row exists to fix.

**And a miss, if one comes back, does not pick its own remedy.** Three and a
half thousand open fronts is a number no list answers at any speed, so whether
that surface should exist in that shape is UX's question, asked separately, and
it can also kill. `open` was rewritten on that argument and never on this one.

### Storage

An append-only log, folded in memory, with a derived index beside it. `events`
is the source of truth and the one file a command may never lose; `index` is a
binary snapshot of a fold that already happened, and **deleting it changes no
output, only how long the next command takes**. That asymmetry is the whole
rule, and it is what separates an index from a second copy of the truth: the
moment the index holds something the log does not, it has stopped being an
index.

- Indexes are thought out from the model, not bolted on when they hurt: the
  queries that matter are *ancestors of a node* and *open descendants of a
  node*, and both are recursive. Measure with a real tree, not with ten toy
  nodes.
- Semantic search stays off the write path and is **always optional**: the
  product has to be complete without it. That is the arbitration above, not a
  storage choice.

**This section used to say SQLite, and the measurement said otherwise.** The
reversal belongs here rather than in a changelog, because this is the document
a rejection cites for its authority, and a document that only records the
rejections it got right is worth less than one that records all of them. The
trigger was real: a log folded on every call held the write budget into the
low thousands of nodes and went over at ten thousand. What the trigger never
settled was the remedy. Measured against the log it already had, SQLite came
out dominated on every axis, and asked for a C compiler, seventeen more crates
and three times the build in exchange. The same measurement found where the
cost actually sits -- not in where the bytes live, but in a node owning its
strings, so that folding allocates one per field -- and no storage engine
fixes that.

### What the budget forbids

- **No network on the write path.** Ever. `push` is the binary writing to a
  file, and nothing in that path waits on anything.

**And one correction, which belongs here because this is the document a
rejection cites for its authority.** *"No daemon on the local path"* used to sit
in this list. It does not belong to the pillar: startup latency is a real cost,
but keeping a process alive or not is a scope and sequencing choice, and quoting
a pillar to end that argument borrows authority the claim never had.

There is still no daemon, and now for a better reason than a budget: the session
hooks are the binary reading a local file, so nothing needs one. An interface
somebody starts by hand, which listens on the loopback address and dies when
they close it, is not a daemon and does not touch this budget. Listening on the
loopback address is also not phoning home — that one belongs to the security
pillar, it is absolute, and it is untouched.

---

## Pillar 3 — Security

This stores the decisions of a development that is very probably private. **The
tree is a map of where a system is weak and not yet fixed.**

### Hard guards

- **Keys and secrets are never stored.** A redaction guard at write time: known
  prefixes (`sqa_`, `squ_`, `ghp_`, `sk-`, `AKIA`, PEM headers) and high-entropy
  strings. In doubt, refuse the write and say why; never store in silence.
- **The user's personal data is never stored.** No email, no name, no home path.
- **File contents are never stored.** Only paths, references and prose about
  what was decided. It is the strongest guard of all because it bounds the blast
  radius of a leak to "what was being worked on", never to "what the code is".
  A write that opens a fenced code block is refused. The fence is the whole rule
  on purpose: there is no `--force` to fall back on, so a rule can only afford to
  have no false positives, and a fence is the writer stating outright that what
  follows is not prose. A paste with no fence gets through, and that is the price
  of having no escape hatch.
- **No telemetry.** The binary does not phone home. Ever.

### Encryption

- **Local**: optional encryption at rest.
- **Cloud / team collaboration**: encryption is **mandatory**, and the question
  to answer before writing a line of that part is **who holds the key**. If the
  server holds it, that is not privacy, it is a promise.
- Identity for team mode: an opaque identifier, never an email or a name.

---

## Pillar 4 — UX

Added on 2026-09-03, when the web became the thing being built. It is not new
ground: it is one sentence that had been sitting inside the DX pillar, and that
sentence had never rejected anything.

DX is the contract with a developer and with a program: a command that scripts,
an exit code, output that parses, capture that costs less than losing the thread.
UX is the other half and its subject is different: **a person looking at a
screen, who does not yet know what to ask.**

### What it costs when it fails, measured

On 2026-09-02 the owner of this project learned in a single day about three
decisions that had been written down for days. Nothing had been lost. The tree
held them, the documents held them, and none of it had arrived. The words that
day were *"we should know it at the same time"*.

That is not a memory failure and it is not a capture failure -- both of those
worked exactly as designed. It is the thing this pillar exists to prevent: **the
gap between what a system holds and what a person has actually seen.**

The CLI answers well when you know what to ask. Until now, nothing in the product
answered the other thing.

### The burden of proof

> **Every reading surface has to say, in one sentence, what a person learns from
> it that they were not going to learn.** The sentence is written before the
> surface is built and it goes on the node. No sentence, no surface.

**"Were not going to learn" covers what was already written and was not going to
be read.** That is the whole failure this pillar came out of: nothing was
missing, and it still did not arrive. Availability is not arrival. So a surface
that carries information the product already holds, and lands it in a glance
where the text needed a scroll and a memory of what came before, has answered the
question honestly. What has not answered it is a surface whose entire case is
that it is more comfortable.

It points the restrictive way: it cuts surfaces, it never licences them. A page
that shows what a command already says is not a second way to read, it is a
second place to keep current -- which is the argument that killed the TUI,
arriving from the other side.

### How it is checked

This is the one pillar whose criterion is not a test, and pretending otherwise
would be worse than saying it plainly. A suite can prove a page renders, that its
numbers are right and that nothing leaked. It cannot prove somebody found out in
time.

So the check is the failure that created the pillar, turned into a question:
**did you learn it while it still mattered?** It is asked of a person, and a "no"
is a finding like any other.

### What it does not get to do

- **It does not touch the security pillar.** A page that would show more is not
  an argument against a veto.
- **It does not soften anything of DX's.** Meaning is still never encoded in
  colour alone, and everything the agent needs is still doable from the command
  line. Those exist so that nobody is locked out, and a better-looking screen is
  not a reason to lock somebody out.
- **It is not a licence to build more.** It is a burden of proof, which means the
  default answer is no.
