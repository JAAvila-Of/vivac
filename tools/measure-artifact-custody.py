#!/usr/bin/env python3
"""Measure the complete Rust capture command on isolated synthetic files."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--temp-dir", type=Path, help="existing parent on the filesystem being measured")
    parser.add_argument("--profile", choices=["debug", "release"], required=True)
    parser.add_argument("--rust-build", required=True, help="compiler version used to build this binary")
    parser.add_argument("--filesystem", required=True, help="verified filesystem of the temporary directory")
    parser.add_argument("--repository", choices=["git", "plain"], default="git",
                        help="include Git preflight checks (default) or measure a plain project")
    args = parser.parse_args()
    if args.samples < 100:
        parser.error("at least 100 samples are required for the p99 comparison")
    binary = args.binary.resolve(strict=True)
    report = {"platform": platform.platform(), "binary": str(binary),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "samples": args.samples, "profile": args.profile,
              "rust_build": args.rust_build, "filesystem": args.filesystem,
              "repository": args.repository,
              "timing": "fresh process, complete capture including synchronized local retention",
              "fixture": "repeated UTF-8 layout prose and separators; exact byte lengths",
              "cells": []}
    temporary_parent = (args.temp_dir or Path(tempfile.gettempdir())).resolve(strict=True)
    if any(any((parent / ".vivac" / marker).exists()
               for marker in ("config", "events", "lane"))
           for parent in (temporary_parent, *temporary_parent.parents)):
        raise RuntimeError("benchmark fixtures must be outside every existing vivac project")
    with tempfile.TemporaryDirectory(prefix="vivac-artifact-budget-", dir=temporary_parent) as folder:
        root = Path(folder)
        project = root / "project"
        project.mkdir()
        if any(any((parent / ".vivac" / marker).exists()
                   for marker in ("config", "events", "lane"))
               for parent in project.parents):
            raise RuntimeError("benchmark fixtures must be outside every existing vivac project")
        report["volume"] = project.anchor
        env = os.environ.copy()
        env.update(VIVAC_HOME=str(root / "registry"), NO_COLOR="1")
        if args.repository == "git":
            subprocess.run(["git", "init", "--quiet"], cwd=project, env=env, check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

        def call(*command):
            result = subprocess.run([str(binary), *command], cwd=project,
                                    env=env, capture_output=True, check=False)
            if result.returncode:
                raise RuntimeError(f"command {command[0]} failed with exit {result.returncode}")
            return result.stdout

        call("init", "--yes")
        if not (project / ".vivac" / "config").is_file() or not (project / ".vivac" / "events").is_file():
            raise RuntimeError("benchmark initialization did not create an independent local tree")
        call("push", "Measure selected local delivery", "--why", "isolated synthetic budget fixture")
        for size, ceiling in [(1024 * 1024, 500), (10 * 1024 * 1024, 2000)]:
            unit = "<div>Reviewed layout with local retained revision.</div>\n".encode()
            body = (unit * (size // len(unit) + 1))[:size]
            (project / "delivery.html").write_bytes(body)
            expected = hashlib.sha256(body).hexdigest()
            elapsed = []
            for sample in range(args.samples):
                started = time.perf_counter_ns()
                reply = json.loads(call("artifacts", "capture", "delivery.html", "--from", "g1", "--json"))
                elapsed.append((time.perf_counter_ns() - started) / 1_000_000)
                receipt = reply["delivery"]
                retained = project / ".vivac-artifacts" / receipt["object"]
                assert receipt["size"] == size and receipt["digest"] == expected
                assert hashlib.sha256(retained.read_bytes()).hexdigest() == expected
                # This isolated benchmark intentionally removes synthetic copies
                # after verifying them, keeping peak disk use below the fixture size.
                retained.unlink()
                if (sample + 1) % 20 == 0:
                    print(f"{size} bytes: {sample + 1}/{args.samples}", flush=True)
            p99 = sorted(elapsed)[math.ceil(0.99 * len(elapsed)) - 1]
            report["cells"].append({"size_bytes": size, "p50_ms": statistics.median(elapsed),
                                    "p99_ms": p99, "max_ms": max(elapsed),
                                    "ceiling_ms": ceiling, "within_budget": p99 < ceiling,
                                    "milliseconds": elapsed})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps([{k: v for k, v in cell.items() if k != "milliseconds"}
                      for cell in report["cells"]], indent=2), flush=True)
    return 0 if all(cell["within_budget"] for cell in report["cells"]) else 1


if __name__ == "__main__":
    raise SystemExit(main())
