#!/usr/bin/env python3
"""Regenerate `tests/data/identifier-vocabulary.txt`.

The guard in `tests/identifiers.rs` is the only implementation of what counts
as an identifier in this crate, and this script does not have a second one: it
runs the guard, takes the block the guard prints when the file is out of date,
and writes that. Nothing here parses Rust, so the two cannot drift apart --
which is the failure the sibling script warns about from the other side.

    python tools/identifier-vocabulary.py

**Read the diff before committing it.** This script blesses whatever the tree
currently calls things, so running it on an unfixed rename is how a Spanish
word gets a permanent licence. The order is the same one
`spanish-vocabulary.py` insists on: fix first, regenerate second.

Two things still catch a bad run afterwards, and neither replaces reading the
diff. `the_vocabulary_carries_no_spanish` refuses any word the output guard
bans, and the file is exactly the tree's vocabulary, so a word that stops
being used disappears instead of lingering with a licence.
"""

import io
import os
import re
import subprocess
import sys

OUT = "tests/data/identifier-vocabulary.txt"
BEGIN = "--- BEGIN VOCABULARY ---"
END = "--- END VOCABULARY ---"


def main():
    if not os.path.isdir("tests/data"):
        sys.exit("run me from the crate root")

    run = subprocess.run(
        ["cargo", "test", "--test", "identifiers", "--",
         "every_identifier_reads_as_english", "--nocapture"],
        capture_output=True, text=True)
    out = run.stdout + run.stderr

    if BEGIN not in out:
        if run.returncode == 0:
            print(f"{OUT} already matches the tree. Nothing to do.")
            return
        sys.exit("the guard failed for some other reason:\n\n" + out)

    body = out.split(BEGIN, 1)[1].split(END, 1)[0]
    words = [w for w in (line.strip() for line in body.splitlines()) if w]
    if not all(re.fullmatch(r"[a-z][a-z0-9]*", w) for w in words):
        sys.exit("the block carries something that is not a word; not writing")

    before = set()
    if os.path.exists(OUT):
        before = set(w.strip() for w in io.open(OUT, encoding="utf-8") if w.strip())
    after = set(words)

    io.open(OUT, "w", encoding="utf-8", newline="\n").write(
        "\n".join(sorted(after)) + "\n")

    added = sorted(after - before)
    dropped = sorted(before - after)
    print(f"{len(after)} words written to {OUT}")
    if added:
        print("  added:   " + ", ".join(added))
    if dropped:
        print("  dropped: " + ", ".join(dropped))
    if added:
        print("\nRead the added words. Anything that is not English means an "
              "identifier to rename,\nnot a line to keep.")


if __name__ == "__main__":
    main()
