#!/usr/bin/env python3
"""Guard the commit messages, which are the third public surface.

`tests/english.rs` guards what the binary prints and `tests/identifiers.rs`
guards the names in the code. Nothing guarded the commit log -- and once the
version number is derived from it, a malformed message does not fail loudly.
It silently does not count, and the release goes out with the old number.

    python3 tools/check-commits.py --self-test
    git log --format='%B%x00' A..B | python3 tools/check-commits.py

Messages arrive NUL separated so a body with blank lines stays one message.

No dependencies, for the reason `src/glob.rs` gives about itself: the
security pillar prefers little to audit, and the rule is a regex. The two
lists it reads are fixtures under `tests/data/`, shared with the guards that
already use them, so a rule lives in one place rather than in each of the
things that enforce it.
"""

import io
import os
import re
import sys

TYPES = ("feat", "fix", "refactor", "perf", "test",
         "docs", "build", "ci", "chore", "revert")

SUBJECT = re.compile(r"^(" + "|".join(TYPES) + r")(\([a-z0-9.-]+\))?!?: (.+)$")

LIMIT = 72

# `era`, `doe`, `yoe`, `doy` and `mp` are Howard Hinnant's names in the
# `civil_from_days` algorithm that `clock.rs` implements; `era` is the one the
# Spanish vocabulary happens to contain. The same exception the identifier
# guard carries, for the same reason.
KNOWN_ENGLISH = {"era"}

GOOD = [
    "fix(cli): refuse an id that names nothing instead of hitting the focus",
    "chore(release): 0.3.2",
    "docs: correct the test count in the status section",
    "feat!: one language. the Spanish compatibility layer is gone",
    "refactor: rename the identifiers d45 left behind\n\n"
    "The rule leaked a second time, and quietly.\n",
    # A body has to be able to name what it changed. This case exists
    # because `prose` once shipped with a dead pattern in it and the suite
    # never noticed: nothing here depended on the filter doing anything.
    "fix(cli): refuse the option the parser used to drop\n\n"
    "I typed --tipo instead of --kind and escribir_crudo moved with it.\n",
]

BAD = [
    ("Fix: refuse an id that names nothing", "the type is capitalised"),
    ("arreglo: rechaza un id que no resuelve", "not one of the valid types"),
    ("feat(cli): add the flag.", "the subject ends with a period"),
    ("feat(cli): aparcar un nodo que no existe", "Spanish in the subject"),
    ("feat: " + "a" * 80, "longer than the limit"),
    ("just some words", "no type at all"),
    ("fix: drop the stale alias\n\naparcar un nodo que no existe\n",
     "Spanish prose in the body, which the filter must not excuse"),
]


def fixture(name):
    here = os.path.dirname(os.path.abspath(__file__))
    path = os.path.join(here, "..", "tests", "data", name)
    with io.open(path, encoding="utf-8") as f:
        return [l.strip() for l in f
                if l.strip() and not l.lstrip().startswith("#")]


def lists():
    spanish = set(fixture("spanish-vocabulary.txt")) - KNOWN_ENGLISH
    banned = [b.lower() for b in fixture("attribution-vocabulary.txt")]
    return spanish, banned


def prose(message):
    """The message with the parts that are not prose taken out.

    A body has to be able to name the thing it changed, and the thing it
    changed is often a Spanish identifier this project is in the middle of
    removing. Measured over the whole history: without this, `--tipo`,
    `escribir_crudo` and `--cascada` all read as Spanish prose, and so does
    the English word TODO. With it, what is left is the handful of commits
    that quote the Spanish they are deleting -- which is a limit of the
    rule, not a defect: a commit about removing Spanish contains Spanish.
    Anything you need to quote goes in backticks.
    """
    message = re.sub(r"`[^`]*`", " ", message)           # quoted spans
    message = re.sub(r"--[A-Za-z0-9_-]+", " ", message)  # flags
    return re.sub(r"\b\w*_\w*\b", " ", message)          # snake_case


def problems(message, spanish, banned):
    """Every rule this message breaks. Empty means it is fine."""
    out = []
    subject = message.strip().split("\n")[0]

    if len(subject) > LIMIT:
        out.append("the subject is %d characters and the limit is %d"
                   % (len(subject), LIMIT))

    m = SUBJECT.match(subject)
    if not m:
        out.append("not a Conventional Commit: expected type(scope): subject, "
                   "with type one of " + ", ".join(TYPES))
    else:
        rest = m.group(3)
        if rest[:1].isupper():
            out.append("the subject starts with a capital letter")
        if rest.endswith("."):
            out.append("the subject ends with a period")

    flat = " ".join(message.lower().split())
    for phrase in banned:
        if phrase in flat:
            out.append("carries an attribution the project does not sign: %r"
                       % phrase)
            break

    for word in re.findall(r"[^\W\d_]+", prose(message).lower()):
        if word in spanish:
            out.append("the Spanish word %r; everything public is in English."
                       " If you are quoting an identifier, put it in backticks"
                       % word)
            break

    return out


def check(messages, spanish, banned):
    bad = 0
    for message in messages:
        found = problems(message, spanish, banned)
        if found:
            bad += 1
            print("\n  %s" % message.strip().split("\n")[0])
            for p in found:
                print("      %s" % p)
    if bad:
        print("\n  %d commit message(s) to rewrite. The version number is "
              "derived from these,\n  so a malformed one does not fail loudly "
              "-- it silently does not count.\n" % bad)
    return 1 if bad else 0


def self_test(spanish, banned):
    # The attribution case is built from the fixture instead of written out,
    # so that this file never has to spell an assistant's name.
    cases = BAD + [("fix: drop the stale alias\n\n%s <someone@example.com>\n"
                    % banned[0], "an attribution taken from the fixture")]
    failed = []
    for message in GOOD:
        found = problems(message, spanish, banned)
        if found:
            failed.append("should have passed: %r -- %s"
                          % (message.split("\n")[0], "; ".join(found)))
    for message, why in cases:
        if not problems(message, spanish, banned):
            failed.append("should have failed (%s): %r"
                          % (why, message.split("\n")[0]))
    for line in failed:
        print("  " + line)
    total = len(GOOD) + len(cases)
    print("  %d of %d cases behaved as written." % (total - len(failed), total))
    return 1 if failed else 0


def main():
    spanish, banned = lists()
    if "--self-test" in sys.argv[1:]:
        return self_test(spanish, banned)
    messages = [m for m in sys.stdin.read().split("\0") if m.strip()]
    if not messages:
        print("  no commit messages on stdin; nothing to check")
        return 0
    return check(messages, spanish, banned)


if __name__ == "__main__":
    sys.exit(main())
