#!/usr/bin/env python3
"""Regenerate `tests/data/spanish-vocabulary.txt`.

The list the output guard reads is derived, not remembered:

    every word the binary printed while it was Spanish
      minus every word it prints today

The Spanish side is lifted from the string literals of commit 4846499, the
last one before the port to English, so that half never changes. The English
side is read from the working tree, which means **a word leaves the list by
being spoken in English** and comes back the moment it stops being.

Run it after fixing a string the guard caught, and commit the result:

    python tools/spanish-vocabulary.py

Order matters. Regenerating *before* the fix would subtract the very word that
is still wrong -- which is how `en_paralelo` stayed out of the list for one
commit after the guard was written.

Kept as a script and not a test because the Spanish half lives in git history,
which the packaged crate does not carry.
"""

import glob
import io
import os
import re
import subprocess
import sys

LAST_SPANISH_COMMIT = "4846499"
OUT = "tests/data/spanish-vocabulary.txt"

# The guard does not get to widen its own list.
SKIP = {"english.rs"}


def literal_spans(src):
    """(start, end) of every string and char literal, comments excluded.

    A rename over these sources has destroyed the flag alias table, the serde
    aliases and a test fixture, three separate times, every one of them with
    the suite green. Nothing here reads a literal by accident.
    """
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c == "r" and i + 1 < n and src[i + 1] in '"#':
            j, hashes = i + 1, 0
            while j < n and src[j] == "#":
                hashes, j = hashes + 1, j + 1
            if j < n and src[j] == '"':
                close = '"' + "#" * hashes
                k = src.find(close, j + 1)
                k = n if k < 0 else k + len(close)
                out.append((i, k))
                i = k
                continue
        if c == '"':
            j = i + 1
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == '"':
                    j += 1
                    break
                j += 1
            out.append((i, j))
            i = j
            continue
        if c == "'":
            m = re.match(r"'(\\.|[^\\'])'", src[i:])
            if m:
                out.append((i, i + m.end()))
                i += m.end()
                continue
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = src.find("\n", i)
            i = n if j < 0 else j
            continue
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            j = src.find("*/", i)
            i = n if j < 0 else j + 2
            continue
        i += 1
    return out


def words_in_literals(src):
    out = set()
    for a, b in literal_spans(src):
        lit = src[a:b]
        if lit.startswith("'"):
            continue
        for w in re.findall(r"[A-Za-zÀ-ſ]{3,}", lit):
            out.add(w.lower())
    return out


def main():
    if not os.path.isdir("tests/data"):
        sys.exit("run me from the crate root")

    spoken_in_spanish = set()
    listing = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", LAST_SPANISH_COMMIT],
        capture_output=True, text=True, check=True).stdout.split()
    for f in listing:
        if f.endswith(".rs"):
            src = subprocess.run(["git", "show", f"{LAST_SPANISH_COMMIT}:{f}"],
                                 capture_output=True, text=True, check=True).stdout
            spoken_in_spanish |= words_in_literals(src)

    spoken_today = set()
    for f in sorted(glob.glob("src/*.rs") + glob.glob("tests/*.rs")
                    + glob.glob("tests/common/*.rs")):
        if os.path.basename(f) in SKIP:
            continue
        spoken_today |= words_in_literals(io.open(f, encoding="utf-8").read())

    banned = sorted(spoken_in_spanish - spoken_today)
    io.open(OUT, "w", encoding="utf-8", newline="\n").write("\n".join(banned) + "\n")
    print(f"{len(spoken_in_spanish)} spoken in Spanish, {len(spoken_today)} spoken today "
          f"-> {len(banned)} banned, written to {OUT}")


if __name__ == "__main__":
    main()
