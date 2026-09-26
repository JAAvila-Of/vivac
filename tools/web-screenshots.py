#!/usr/bin/env python3
"""Rebuild the screenshots of `vivac web` that the README shows.

    python3 tools/web-screenshots.py [--vivac PATH] [--chrome PATH] [--out DIR]

The pictures are of example projects and never of a real tree: a real tree
names real projects, and the web's index would show every tree this machine
knows. So this plants four made-up projects in a throwaway directory, with
`VIVAC_HOME` pointed inside it so the machine's own registry is never read or
written, and photographs what the installed binary serves for them.

Every node is written by a real `vivac` command. The one thing done by hand
is the dates. `init` stamps today and `import` keeps only a bare date, so no
command can plant a project that has been sitting still for a week, and an
index where everything moved today does not show which project moved and
which did not. Each project is therefore written in stretches, and after
writing, every event of a stretch has the same number of days taken off its
`ts`. Order is kept, nothing else in the log is touched, and if the log is
not the JSON lines this expects the script stops rather than guess: the log
format is not a promise (`docs/USAGE.md`), and this script is not either.

The one-time link `vivac web` prints only ever lands on the index or on a
project's page, and the session it opens lives in a cookie that dies with the
browser. So the script spends the link itself, asks the server for each page
with that cookie, and points the browser at the page saved exactly as the
binary served it: every page inlines its style and script, so nothing else is
needed to draw it.

A pair per page, light and dark, for the README's `<picture>`. Needs Chrome
or Edge; no other dependency, for the reason `check-commits.py` gives.
"""

import argparse
import datetime as dt
import http.client
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

# One project is a list of stretches: how many days ago it happened, and the
# commands written in it, oldest stretch first. Node numbers are the order of
# creation, the same way `vivac` hands them out.
PROJECTS = {
    "billing-api": [
        (9, [
            ["push", "Ship the 2.0 API", "--root", "--type", "goal",
             "--why", "the first customer is waiting on it"],
            ["add", "Security: vetoes on the spot", "--root", "--type", "pillar",
             "--why", "the service holds payment data"],
            ["add", "Never store a card number", "--parent", "2", "--type", "rule",
             "--why", "a leak cannot be taken back"],
            ["push", "Replace the cache adapter",
             "--why", "the session bug traces back to it"],
            ["add", "Sessions expire at 300s, not 3600", "--type", "finding",
             "--why", "reproduced on staging"],
            ["done", "5", "Record: the adapter ignores the configured TTL"],
        ]),
        (4, [
            ["decide", "Retry policy: three tries, then fail loudly",
             "--reason", "silent retries hid the session bug for a month",
             "--alternative", "retry forever with backoff"],
            ["add", "Rate limiting is undecided", "--parent", "1",
             "--why", "the gateway team has not answered"],
            ["park", "7", "not now, after the release"],
        ]),
        (1, [
            ["save", "adapter swapped in", "--next", "migrate the callers"],
        ]),
        (0, [
            ["push", "Migrate the callers",
             "--why", "the old adapter had a different signature"],
            ["add", "Two callers pass the TTL in minutes", "--type", "finding",
             "--why", "the nightly job and the webhook retry"],
            ["decide", "Convert at the boundary, not in each caller",
             "--reason", "one place to test",
             "--alternative", "fix each caller"],
            ["push", "Convert the webhook caller",
             "--why", "the one that fails in production"],
            ["pop", "converted, and covered by a test"],
        ]),
    ],
    "field-app": [
        (2, [
            ["push", "Offline mode for the field app", "--root", "--type", "goal",
             "--why", "technicians lose signal on site"],
            ["push", "Queue writes while offline",
             "--why", "a lost form is a lost visit"],
            ["decide", "Queue on disk, not in memory",
             "--reason", "the phone kills the app in the background",
             "--alternative", "an in-memory queue"],
            ["add", "Sync conflicts have no owner", "--parent", "1",
             "--why", "nobody decided which side wins"],
            ["save", "the queue works on one device",
             "--next", "test on the oldest supported phone"],
        ]),
    ],
    "ci-costs": [
        (6, [
            ["push", "Cut the CI bill in half", "--root", "--type", "goal",
             "--why", "it doubled in one quarter"],
            ["push", "Cache the dependency builds",
             "--why", "cold builds are most of the minutes"],
            ["save", "cache on main", "--next", "measure a week of runs"],
        ]),
        (1, [
            ["add", "Cold builds take 4 min, warm ones 40 s", "--type", "finding",
             "--why", "a week of runs on main"],
            ["done", "3", "Record: 4 min cold, 40 s warm, on the default runner"],
            ["push", "Share the cache across branches",
             "--why", "branches still build cold"],
        ]),
    ],
    "docs-site": [
        (13, [
            ["push", "Move the docs to the new generator", "--root", "--type", "goal",
             "--why", "the old one is no longer maintained"],
            ["push", "Port the search index",
             "--why", "the new generator ships none"],
            ["add", "The old index only covered English", "--type", "finding",
             "--why", "measured on the live site"],
        ]),
    ],
}

# The project whose pages are shown, and each page: its path on the server
# and how tall a window it needs.
FEATURED = "billing-api"
PAGES = [
    ("index", "/", 500),
    ("today", f"/p/{FEATURED}/", 1250),
    ("tree", f"/p/{FEATURED}/tree", 900),
]

SCHEMES = {"light": 1, "dark": 0}

WIDTH = 1100
SCALE = 2


def run(vivac, args, cwd, env):
    out = subprocess.run([vivac, *args], cwd=cwd, env=env,
                         capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"`vivac {' '.join(args)}` failed in {cwd}:\n"
                 f"{out.stdout}{out.stderr}")


def event_count(root):
    with open(os.path.join(root, ".vivac", "events"), encoding="utf-8") as f:
        return sum(1 for line in f if line.strip())


def shift_dates(root, stretches):
    """Take `days` off the `ts` of every event in each stretch.

    `stretches` is `(days, first_line, end_line)`, end exclusive, in log order.
    """
    path = os.path.join(root, ".vivac", "events")
    with open(path, encoding="utf-8") as f:
        lines = [line for line in f.read().split("\n") if line.strip()]
    stamp = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
    for days, first, end in stretches:
        for i in range(first, end):
            event = json.loads(lines[i])
            ts = event.get("ts")
            if not isinstance(ts, str) or not stamp.match(ts):
                sys.exit(f"{path}: line {i + 1} has no ts this script "
                         "understands; the log format moved, so this must too")
            when = dt.datetime.strptime(ts, "%Y-%m-%dT%H:%M:%SZ")
            event["ts"] = (when - dt.timedelta(days=days)).strftime(
                "%Y-%m-%dT%H:%M:%SZ")
            lines[i] = json.dumps(event, ensure_ascii=False,
                                  separators=(",", ":"))
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


def plant(vivac, base, env):
    for name, stretches in PROJECTS.items():
        root = os.path.join(base, name)
        os.makedirs(root)
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        run(vivac, ["init", "--yes"], root, env)
        marks = []
        first = 0
        for days, commands in stretches:
            for args in commands:
                run(vivac, args, root, env)
            end = event_count(root)
            marks.append((days, first, end))
            first = end
        shift_dates(root, marks)


def serve(vivac, cwd, env):
    """Start `vivac web` and return it, its port, and the session cookie."""
    server = subprocess.Popen([vivac, "web", "--no-open", "--port", "0"],
                              cwd=cwd, env=env, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True)
    deadline = time.time() + 10
    while time.time() < deadline:
        line = server.stdout.readline()
        found = re.search(r"http://127\.0\.0\.1:(\d+)(/\?k=[0-9a-f]+)", line)
        if found:
            port = int(found.group(1))
            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
            conn.request("GET", found.group(2))
            answer = conn.getresponse()
            jar = answer.getheader("Set-Cookie") or ""
            conn.close()
            if answer.status != 302 or not jar:
                server.terminate()
                sys.exit(f"the link was not taken: {answer.status}")
            return server, port, jar.split(";")[0]
    server.terminate()
    sys.exit("vivac web never printed its link")


def fetch(port, path, cookie):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    conn.request("GET", path, headers={"Cookie": cookie})
    answer = conn.getresponse()
    body = answer.read()
    conn.close()
    if answer.status != 200:
        sys.exit(f"{path} answered {answer.status}")
    return body


def shoot(chrome, page, scheme, height, profile, target):
    subprocess.run([
        chrome, "--headless=new", "--disable-gpu", "--hide-scrollbars",
        f"--user-data-dir={profile}",
        f"--blink-settings=preferredColorScheme={SCHEMES[scheme]}",
        f"--window-size={WIDTH},{height}",
        f"--force-device-scale-factor={SCALE}",
        "--virtual-time-budget=3000",
        f"--screenshot={target}", "file:///" + page.replace(os.sep, "/"),
    ], check=True, capture_output=True)


def find_chrome():
    for candidate in (
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        shutil.which("google-chrome") or "",
        shutil.which("chromium") or "",
    ):
        if candidate and os.path.exists(candidate):
            return candidate
    sys.exit("no Chrome or Edge found; pass --chrome")


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--vivac", default=shutil.which("vivac"))
    parser.add_argument("--chrome")
    parser.add_argument("--out", default=os.path.join(here, "..", "docs", "img"))
    a = parser.parse_args()
    if not a.vivac:
        sys.exit("no vivac on the PATH; pass --vivac")
    chrome = a.chrome or find_chrome()
    version = subprocess.run([a.vivac, "--version"], capture_output=True,
                             text=True).stdout.strip()

    base = tempfile.mkdtemp(prefix="vivac-screens-")
    try:
        env = dict(os.environ, VIVAC_HOME=os.path.join(base, ".home"))
        plant(a.vivac, os.path.join(base, "projects"), env)
        server, port, cookie = serve(a.vivac, base, env)
        try:
            for page, path, height in PAGES:
                saved = os.path.join(base, f"{page}.html")
                with open(saved, "wb") as f:
                    f.write(fetch(port, path, cookie))
                for scheme in SCHEMES:
                    target = os.path.abspath(
                        os.path.join(a.out, f"web-{page}-{scheme}.png"))
                    profile = os.path.join(base, f".chrome-{page}-{scheme}")
                    shoot(chrome, saved, scheme, height, profile, target)
                    print(f"  {target}")
        finally:
            server.terminate()
            server.wait()
        print(f"  taken with {version}")
    finally:
        shutil.rmtree(base, ignore_errors=True)


if __name__ == "__main__":
    main()
