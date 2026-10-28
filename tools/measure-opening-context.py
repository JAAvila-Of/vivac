#!/usr/bin/env python3
"""Measure complete fresh-process session opening producers on a synthetic tree."""

import argparse
from datetime import datetime, timezone
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


LIMIT_BYTES = 60_000
FIXTURE_SEED = 20261025
MODES = {
    "legacy": ("session", "start", "--hook"),
    "codex": ("session", "start", "--hook", "--context", "codex"),
    "claude-module": ("session", "start", "--hook", "--context", "claude-module"),
}
SYNTHETIC_TS = "2026-01-01T00:00:00Z"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def percentile(values, percentile_value):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(percentile_value * len(ordered)) - 1)]


def refuse_existing_tree(path):
    for parent in (path, *path.parents):
        vivac = parent / ".vivac"
        if any((vivac / marker).exists() for marker in ("config", "events", "lane")):
            raise RuntimeError("benchmark fixtures must be outside every existing vivac project")


def event(seq, event_id, actor, lane, payload):
    return {"seq": seq, "id": event_id, "ts": SYNTHETIC_TS,
            "actor": actor, "lane": lane, "payload": payload}


def build_events(binary, project, env, nodes, fronts):
    initialized = subprocess.run(
        [str(binary), "init", "--yes"], cwd=project, env=env,
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
    )
    if initialized.returncode:
        detail = initialized.stderr.decode("utf-8", "replace")[-1000:]
        raise RuntimeError(f"synthetic tree initialization failed ({initialized.returncode}): {detail}")

    event_path = project / ".vivac" / "events"
    if not event_path.is_file():
        raise RuntimeError("tree initialization did not create an event log")
    original = event_path.read_bytes()
    base_lines = [line for line in original.splitlines() if line]
    base_events = [json.loads(line) for line in base_lines]
    lane_event = next((item for item in base_events
                       if item.get("payload", {}).get("type") == "lane.declared"), None)
    if lane_event is None:
        raise RuntimeError("initialized tree has no lane.declared event")

    actor = lane_event["actor"]
    lane = lane_event["lane"]
    seq = max(item["seq"] for item in base_events) + 1
    lines = list(base_lines)
    internal = nodes - fronts
    for num in range(1, nodes + 1):
        node_id = f"n{num}"
        if num == 1:
            kind, parent = "goal", None
        elif num <= internal:
            kind, parent = "task", f"n{max(1, num // 2)}"
        else:
            kind = "task"
            parent_num = 1 if internal == 1 else 2 + ((num - internal - 1) % (internal - 1))
            parent = f"n{parent_num}"
        payload = {
            "type": "node.created", "node": node_id, "num": num,
            "kind": kind, "title": f"Synthetic node {num}",
                "why": f"Deterministic opening benchmark fixture; seed {FIXTURE_SEED}.", "parent": parent,
            "blocks": False, "refs": [], "governs": [],
        }
        lines.append(json.dumps(event(seq, f"e{seq}", actor, lane, payload),
                                separators=(",", ":"), ensure_ascii=False).encode("utf-8"))
        seq += 1
        if num <= internal:
            closed = {"type": "state.changed", "node": node_id, "state": "done",
                      "outcome": "Synthetic closed parent."}
            lines.append(json.dumps(event(seq, f"e{seq}", actor, lane, closed),
                                    separators=(",", ":"), ensure_ascii=False).encode("utf-8"))
            seq += 1

    focus = {"type": "stack.pushed", "node": f"n{nodes}"}
    lines.append(json.dumps(event(seq, f"e{seq}", actor, lane, focus),
                            separators=(",", ":"), ensure_ascii=False).encode("utf-8"))
    fixture = b"\n".join(lines) + b"\n"
    event_path.write_bytes(fixture)
    scaffold = b"\n".join(base_lines) + b"\n"
    return event_path, fixture, scaffold


def context_from(mode, stdout):
    if mode == "legacy":
        try:
            stdout.decode("utf-8")
        except UnicodeDecodeError as error:
            raise RuntimeError("legacy producer did not return valid UTF-8 context") from error
        return stdout
    try:
        response = json.loads(stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"{mode} producer did not return valid UTF-8 JSON") from error
    if mode == "codex":
        if response.get("hookSpecificOutput", {}).get("hookEventName") != "SessionStart":
            raise RuntimeError("Codex producer did not return a ready SessionStart envelope")
        context = response["hookSpecificOutput"].get("additionalContext")
    else:
        if (response.get("protocol") != "vivac.claude.opening.v1"
                or response.get("status") != "ready"):
            raise RuntimeError("Claude module producer did not return a ready envelope")
        context = response.get("context")
    if not isinstance(context, str):
        raise RuntimeError(f"{mode} producer returned no complete text context")
    return context.encode("utf-8")


def run_once(binary, project, env, mode):
    started = time.perf_counter_ns()
    result = subprocess.run(
        [str(binary), *MODES[mode]], cwd=project, env=env, input=b"{}",
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
    )
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    if result.returncode:
        detail = result.stderr.decode("utf-8", "replace")[-1000:]
        raise RuntimeError(f"{mode} producer failed ({result.returncode}): {detail}")
    if result.stderr:
        detail = result.stderr.decode("utf-8", "replace")[-1000:]
        raise RuntimeError(f"{mode} producer wrote to stderr: {detail}")
    context = context_from(mode, result.stdout)
    if len(context) > LIMIT_BYTES:
        raise RuntimeError(f"{mode} context is {len(context)} bytes; limit is {LIMIT_BYTES}")
    return elapsed_ms, context


def source_revision(binary):
    for parent in (binary.parent, *binary.parents):
        if (parent / ".git").exists():
            top = subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=parent,
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False)
            if top.returncode:
                break
            root = Path(top.stdout.decode("utf-8", "replace").strip())
            revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root,
                                      stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=False)
            status = subprocess.run(["git", "status", "--porcelain=v1", "--untracked-files=all"],
                                    cwd=root, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                    check=False)
            diff = subprocess.run(["git", "diff", "HEAD", "--", "*.rs", "Cargo.toml", "Cargo.lock"],
                                  cwd=root, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                  check=False)
            if revision.returncode or status.returncode or diff.returncode:
                break
            diff_hash = hashlib.sha256(diff.stdout)
            status_lines = status.stdout.decode("utf-8", "replace").splitlines()
            for line in status_lines:
                if not line.startswith("?? "):
                    continue
                relative = line[3:]
                path = root / relative
                if relative.endswith(".rs") or Path(relative).name in {"Cargo.toml", "Cargo.lock"}:
                    if path.is_file():
                        diff_hash.update(relative.encode("utf-8", "surrogateescape"))
                        diff_hash.update(path.read_bytes())
            return {
                "revision": revision.stdout.decode("ascii", "replace").strip(),
                "dirty": bool(status.stdout.strip()),
                "rust_cargo_diff_sha256": diff_hash.hexdigest(),
            }
    return {"revision": None, "dirty": None, "rust_cargo_diff_sha256": None}


def describe_binary(path, minimum_age):
    binary = path.resolve(strict=True)
    if not binary.is_file():
        raise ValueError(f"binary path is not a file: {binary.name}")
    stat = binary.stat()
    age = max(0, int(time.time() - stat.st_mtime))
    if age < minimum_age:
        raise ValueError(
            f"{binary.name} file age is {age}s; minimum is {minimum_age}s; "
            "build-age is not inferred from mtime"
        )
    version_run = subprocess.run([str(binary), "--version"], capture_output=True, check=False)
    if version_run.returncode:
        raise ValueError(f"could not read version from {binary.name}")
    return binary, {
        "name": binary.name,
        "sha256": digest(binary.read_bytes()),
        "version": version_run.stdout.decode("utf-8", "replace").strip(),
        "mtime_utc": datetime.fromtimestamp(stat.st_mtime, timezone.utc).isoformat(),
        "file_age_seconds": age,
        "checkout": source_revision(binary),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True, help="vivac executable to measure")
    parser.add_argument("--baseline-binary", type=Path,
                        help="optional previous-revision executable, measured on the same fixture")
    parser.add_argument("--samples", type=int, default=200)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--temp-dir", type=Path, help="existing parent on the filesystem being measured")
    parser.add_argument("--output", type=Path, help="optional JSON report destination")
    parser.add_argument("--nodes", type=int, default=10_000)
    parser.add_argument("--fronts", type=int, default=100)
    parser.add_argument("--min-binary-age-seconds", type=int, default=300,
                        help="refuse a younger executable; this does not wait")
    args = parser.parse_args()
    if args.samples < 100:
        parser.error("at least 100 measured samples are required for p99")
    if args.warmup < 0:
        parser.error("warmup must be zero or greater")
    if args.nodes < 2 or not 1 <= args.fronts < args.nodes:
        parser.error("require nodes >= 2 and 1 <= fronts < nodes")
    if args.min_binary_age_seconds < 300:
        parser.error("min-binary-age-seconds must be at least 300")

    try:
        candidate, candidate_info = describe_binary(args.binary, args.min_binary_age_seconds)
        binaries = {"candidate": candidate}
        binary_info = {"candidate": candidate_info}
        if args.baseline_binary:
            baseline, baseline_info = describe_binary(args.baseline_binary, args.min_binary_age_seconds)
            if candidate == baseline:
                parser.error("--binary and --baseline-binary must resolve to different files")
            binaries["baseline"] = baseline
            binary_info["baseline"] = baseline_info
    except (OSError, ValueError) as error:
        parser.error(str(error))
    cell_names = ([f"{build}/{mode}" for build in binaries for mode in MODES]
                  if "baseline" in binaries else list(MODES))

    temp_parent = (args.temp_dir or Path(tempfile.gettempdir())).resolve(strict=True)
    if not temp_parent.is_dir():
        parser.error("--temp-dir must be an existing directory")
    refuse_existing_tree(temp_parent)
    env = os.environ.copy()

    report = {
        "schema": 1,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(),
        "volume": temp_parent.anchor,
        "binary_name": candidate_info["name"],
        "binary_sha256": candidate_info["sha256"],
        "binary_version": candidate_info["version"],
        "binary_mtime_utc": candidate_info["mtime_utc"],
        "binary_file_age_seconds": candidate_info["file_age_seconds"],
        "checkout": candidate_info["checkout"],
        "binaries": binary_info,
        "build_provenance": "unknown; checkout revision and dirty state do not identify the source used to build this executable",
        "samples_per_mode": args.samples,
        "warmups_per_mode": args.warmup,
        "fixture": {"seed": FIXTURE_SEED, "nodes": args.nodes, "open_fronts": args.fronts,
                    "format": "synthetic current-format events; initialized lane scaffold",
                    "hash_note": "The event-log snapshot hash includes init-generated actor and lane identifiers, so it can vary between runs."},
        "timing_scope": (
            "fresh subprocess wall time including OS/process startup, tree loading, "
            "index handling, custody reconciliation, opening receipt and output serialization; "
            "no startup subtraction"
        ),
        "reception": "not measured; producer output is not evidence of native harness reception",
        "temporal_budget": "no opening latency ceiling is asserted by this helper",
        "modes": {},
    }

    with tempfile.TemporaryDirectory(prefix="vivac-opening-context-", dir=temp_parent) as folder:
        root = Path(folder)
        project = root / "project"
        project.mkdir()
        refuse_existing_tree(project)
        run_env = env.copy()
        run_env.update(VIVAC_HOME=str(root / "registry"), NO_COLOR="1")
        event_path, fixture_bytes, scaffold_bytes = build_events(
            candidate, project, run_env, args.nodes, args.fronts
        )
        fixture_line_count = len(fixture_bytes.splitlines())
        fixture_hash_before = digest(event_path.read_bytes())
        if fixture_hash_before != digest(fixture_bytes):
            raise RuntimeError("synthetic fixture did not match its expected snapshot before measurement")

        cold_elapsed, cold_context = run_once(candidate, project, run_env, "legacy")
        cold_opening = {
            "mode": "legacy",
            "elapsed_ms": cold_elapsed,
            "context_utf8_bytes": len(cold_context),
            "context_sha256": digest(cold_context),
            "included_in_percentiles": False,
        }
        expected_context = cold_context
        for warmup in range(args.warmup):
            order = list(cell_names)
            shift = warmup % len(order)
            order = order[shift:] + order[:shift]
            for cell in order:
                build, mode = cell.split("/", 1) if "/" in cell else ("candidate", cell)
                _, context = run_once(binaries[build], project, run_env, mode)
                if context != expected_context:
                    raise RuntimeError(f"{cell} context differs from candidate/legacy during warmup")

        elapsed = {cell: [] for cell in cell_names}
        context_info = {cell: None for cell in cell_names}
        for sample in range(args.samples):
            order = list(cell_names)
            shift = sample % len(order)
            order = order[shift:] + order[:shift]
            for cell in order:
                build, mode = cell.split("/", 1) if "/" in cell else ("candidate", cell)
                duration, context = run_once(binaries[build], project, run_env, mode)
                if context != expected_context:
                    raise RuntimeError(f"{cell} context differs from candidate/legacy at sample {sample + 1}")
                elapsed[cell].append(duration)
                info = {"utf8_bytes": len(context), "sha256": digest(context)}
                if context_info[cell] is None:
                    context_info[cell] = info
                elif context_info[cell] != info:
                    raise RuntimeError(f"{cell} output length or digest changed during measurement")

        # Read and verify the immutable synthetic prefix only after all timers stop.
        final_log = event_path.read_bytes()
        if not final_log.startswith(fixture_bytes):
            raise RuntimeError("synthetic event-log prefix changed during measurement")
        appended = [line for line in final_log[len(fixture_bytes):].splitlines() if line]
        appended_events = [json.loads(line) for line in appended]
        if len(appended_events) != 1 + (args.warmup + args.samples) * len(cell_names):
            raise RuntimeError("unexpected opening receipt count")
        if any(item.get("payload", {}).get("type") != "session.started" for item in appended_events):
            raise RuntimeError("opening run appended a non-session event")
        if digest(fixture_bytes) != digest(final_log[:len(fixture_bytes)]):
            raise RuntimeError("synthetic fixture hash changed")

        report["fixture"].update({
            "seed": FIXTURE_SEED,
            "initialized_scaffold_sha256": digest(scaffold_bytes),
            "event_lines_before_measurement": fixture_line_count,
            "snapshot_sha256_before_measurement": fixture_hash_before,
            "snapshot_sha256_verified_after_measurement": digest(final_log[:len(fixture_bytes)]),
            "opening_receipts_appended": len(appended_events),
        })
        report["cold_first_opening"] = cold_opening
        report["steady_state"] = args.warmup > 0
        report["steady_state_note"] = (
            "Measured percentiles follow at least one full warmup cycle per mode."
            if args.warmup > 0 else
            "Warmup is disabled; measured percentiles are not labelled steady-state."
        )
        for cell in cell_names:
            values = elapsed[cell]
            report["modes"][cell] = {
                **context_info[cell],
                "p50_ms": statistics.median(values),
                "p95_ms": percentile(values, 0.95),
                "p99_ms": percentile(values, 0.99),
                "max_ms": max(values),
                "milliseconds": values,
            }

    for build, binary in binaries.items():
        if digest(binary.read_bytes()) != binary_info[build]["sha256"]:
            raise RuntimeError(f"{build} executable changed during measurement")
    report["executables_unchanged_after_measurement"] = True
    serialized = json.dumps(report, indent=2, ensure_ascii=False) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(serialized, encoding="utf-8")
    summary = {key: value for key, value in report.items() if key != "modes"}
    summary["modes"] = {
        mode: {key: value for key, value in values.items() if key != "milliseconds"}
        for mode, values in report["modes"].items()
    }
    print(json.dumps(summary, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
