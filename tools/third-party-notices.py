#!/usr/bin/env python3
"""Write the notices of every crate linked into one release binary.

    python3 tools/third-party-notices.py --target x86_64-unknown-linux-musl

Every release archive carries a `vivac` that has its dependencies compiled
into it, and their licences, MIT and Apache-2.0 among them, ask for their
notices to travel with every copy. So each archive gets a
`THIRD-PARTY-NOTICES` beside the binary, and this prints it.

What counts is what the binary links, read off `cargo metadata` for the
target being packed: the normal dependencies of `vivac`, followed down. A
procedural macro runs while compiling and none of its code reaches the
binary, so it is left out, and so is everything only it depends on. Each
crate contributes every licence and notice file it ships (`LICENSE*`,
`LICENCE*`, `COPYING*`, `NOTICE*`), and a text shared by several crates is
printed once, under all of their names.

It stops, rather than printing a partial file, when a crate ships no licence
file at all: that crate has to be looked at by a person, because an archive
missing a notice is the failure this exists to prevent.

The output is not committed. A generated file checked in is a copy that goes
stale; the release step writes it fresh for each archive. No dependencies,
for the reason `check-commits.py` gives.
"""

import argparse
import json
import os
import subprocess
import sys

PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE")


def metadata(target):
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked",
         "--filter-platform", target],
        capture_output=True, text=True, encoding="utf-8")
    if out.returncode != 0:
        sys.exit(f"cargo metadata failed:\n{out.stderr}")
    return json.loads(out.stdout)


def linked(meta):
    """The packages the binary links, `vivac` itself excluded."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]

    def is_macro(pid):
        return all("proc-macro" in t["kind"] for t in packages[pid]["targets"]
                   if "lib" in t["kind"] or "proc-macro" in t["kind"])

    seen = set()
    todo = [root]
    while todo:
        pid = todo.pop()
        for dep in nodes[pid]["deps"]:
            normal = any(k["kind"] is None for k in dep["dep_kinds"])
            if not normal or dep["pkg"] in seen or is_macro(dep["pkg"]):
                continue
            seen.add(dep["pkg"])
            todo.append(dep["pkg"])
    return sorted((packages[pid] for pid in seen),
                  key=lambda p: (p["name"], p["version"]))


def notice_files(package):
    folder = os.path.dirname(package["manifest_path"])
    found = sorted(
        name for name in os.listdir(folder)
        if name.upper().startswith(PREFIXES)
        and os.path.isfile(os.path.join(folder, name)))
    if package.get("license_file"):
        named = os.path.normpath(package["license_file"])
        if named not in (os.path.normpath(f) for f in found):
            found.append(named)
    return [os.path.join(folder, name) for name in found]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--target", required=True)
    a = parser.parse_args()

    crates = linked(metadata(a.target))
    texts = {}
    order = []
    missing = []
    for package in crates:
        label = f"{package['name']} {package['version']}"
        files = notice_files(package)
        if not files:
            missing.append(f"{label} ({package.get('license') or 'no licence'})")
            continue
        for path in files:
            with open(path, encoding="utf-8", errors="replace") as f:
                text = f.read().strip()
            if text not in texts:
                texts[text] = []
                order.append(text)
            texts[text].append(f"{label}, {os.path.basename(path)}")
    if missing:
        sys.exit("these crates ship no licence file, and one of their notices "
                 "would be missing from the archive:\n  " + "\n  ".join(missing))

    out = sys.stdout
    out.reconfigure(encoding="utf-8", newline="\n")
    out.write("Third-party notices for the vivac binary built for "
              f"{a.target}.\n\n")
    out.write("vivac itself is MIT OR Apache-2.0: see LICENSE-MIT and "
              "LICENSE-APACHE beside this file.\nIt links the crates below, "
              "each under the licence it declares.\n\n")
    for package in crates:
        out.write(f"  {package['name']} {package['version']}  "
                  f"{package.get('license') or ''}\n")
    for text in order:
        out.write("\n" + "=" * 72 + "\n")
        for who in texts[text]:
            out.write(f"{who}\n")
        out.write("-" * 72 + "\n\n")
        out.write(text + "\n")


if __name__ == "__main__":
    main()
