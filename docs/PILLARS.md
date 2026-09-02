# The three pillars

**They govern by definition.** Every design decision is justified against them,
and if it contradicts them it does not get in. They are not aspirations: they
are rejection criteria.

## How they arbitrate between themselves

They will collide. When they do:

| Pillar | Power |
|---|---|
| **Security** | **veto**. It can kill an entire feature, with no negotiation |
| **Performance** | **budget**. It sets a ceiling the feature must respect in order to exist |
| **DX** | **judge**. It decides between the options that already cleared the other two |

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

The people who use this are developers. It has to be intuitive for them and
**visually good**: a well-designed TUI, not a text dump.

### Two audiences, and this is the design fork

| Who | What they do | What they need |
|---|---|---|
| **The agent** (executor) | writes: opens nodes, closes them, notes | **CLI**: quiet, scriptable, exit codes, parseable output |
| **The maintainer** (human) | reads: navigates, understands, decides | **TUI**: the tree, the `why` path, filters, navigation |

**A rule with teeth: everything the agent needs to do has to be doable without
the TUI.** If a feature only exists in the interactive interface, an agent
cannot use it and the project loses half its users. The TUI is never the only
road to anything.

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
| `why` / `tree` over 10,000 nodes | **< 50 ms** | it is interactive reading; above that you feel it |
| text search | **< 100 ms** | same |
| semantic search | off the critical path | see the arbitration above |

### Storage

SQLite, and it should be squeezed rather than fought:

- **WAL** so that reading does not block writing.
- **FTS5** for text. It is what will answer most real searches.
- **Vector** (`sqlite-vec` or equivalent) for the semantic part, and **always
  optional**: the product has to be complete without it.
- Indexes are thought out from the model, not bolted on when they hurt: the
  queries that matter are *ancestors of a node* and *open descendants of a
  node*, and both are recursive. Measure with a real tree, not with ten toy
  nodes.

Until size demands otherwise, the store is an append-only log folded in memory.
Measured on 2026-08-31: it holds the write budget up to the order of a thousand
nodes, and at ten thousand it goes over. That is the trigger for SQLite, and now
it is a number.

### Anti-features

- **No daemon** on the local path: it is startup latency and one more thing to
  fall over.
- **No network on the write path.** Ever.

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
- **No telemetry.** The binary does not phone home. Ever.

### Encryption

- **Local**: optional encryption at rest.
- **Cloud / team collaboration**: encryption is **mandatory**, and the question
  to answer before writing a line of that part is **who holds the key**. If the
  server holds it, that is not privacy, it is a promise.
- Identity for team mode: an opaque identifier, never an email or a name.
