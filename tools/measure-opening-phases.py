#!/usr/bin/env python3
"""Build and run a cfg(test)-free opening phase benchmark in a private copy.

Example:
  python tools/measure-opening-phases.py prepare --build-dir H:/TEMP/vivac-opening-release
  # Build that copy with the project's coordinated Rust launcher:
  # cargo build --locked --release --bin vivac
  python tools/measure-opening-phases.py run --build-dir H:/TEMP/vivac-opening-release \
      --fixture-root H:/TEMP/vivac-opening-fixtures --output H:/TEMP/opening-phases.json \
      --filesystem NTFS --rust-build "rustc 1.94.1; release; cfg(test)=false"
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


REPO = Path(__file__).resolve().parents[1]
DRIVER = Path(__file__).with_name("opening-phases.rs")
MARKER = "VIVAC_OPENING_PHASE_DRIVER"
MIN_BINARY_AGE_SECONDS = 300


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def tree_digest(root: Path, include_manifest: bool = True) -> tuple[str, list[dict[str, str]]]:
    rows = []
    paths = [p for p in root.rglob("*") if p.is_file()]
    paths = [p for p in paths if "target" not in p.relative_to(root).parts]
    if not include_manifest:
        paths = [p for p in paths if p.name != "opening-phase-manifest.json"]
    for path in sorted(paths):
        rel = path.relative_to(root).as_posix()
        rows.append({"path": rel, "sha256": digest(path)})
    encoded = json.dumps(rows, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest(), rows


def compile_input_digest(root: Path) -> tuple[str, list[dict[str, str]]]:
    paths = [root / "Cargo.toml", root / "Cargo.lock"]
    for name in ("src", "vendor"):
        path = root / name
        if path.is_file():
            paths.append(path)
        elif path.is_dir():
            paths.extend(p for p in path.rglob("*") if p.is_file())
    paths.extend(
        path for path in (root / ".cargo" / "config", root / ".cargo" / "config.toml")
        if path.is_file()
    )
    media = root / "tests" / "data" / "artifact-media"
    if media.is_dir():
        paths.extend(p for p in media.rglob("*") if p.is_file())
    rows = [
        {"path": p.relative_to(root).as_posix(), "sha256": digest(p)}
        for p in sorted(paths) if p.is_file()
    ]
    encoded = json.dumps(rows, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest(), rows


def safe_build_dir(raw: str) -> Path:
    path = Path(raw).expanduser().resolve()
    if path == REPO or REPO in path.parents:
        raise SystemExit("build-dir must be outside the live vivac checkout")
    for parent in (path, *path.parents):
        if (parent / ".vivac").exists():
            raise SystemExit("build-dir cannot be beneath a live .vivac tree")
    if path.exists():
        raise SystemExit(f"build-dir must be a new, nonexistent directory: {path}")
    return path


def prepare(args: argparse.Namespace) -> None:
    build = safe_build_dir(args.build_dir)
    build.mkdir(parents=True)
    for name in ("Cargo.toml", "Cargo.lock"):
        shutil.copy2(REPO / name, build / name)
    for name in ("src", "vendor"):
        shutil.copytree(REPO / name, build / name)
    for name in ("config", "config.toml"):
        cargo_config = REPO / ".cargo" / name
        if cargo_config.is_file():
            target = build / ".cargo" / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(cargo_config, target)
    media = REPO / "tests" / "data" / "artifact-media"
    if media.exists():
        shutil.copytree(media, build / "tests" / "data" / "artifact-media")

    session = build / "src" / "session.rs"
    source = session.read_text(encoding="utf-8")
    anchor = "mod opening;\n"
    if source.count(anchor) != 1:
        raise SystemExit("could not locate the unique session module insertion point")
    session.write_text(source.replace(anchor, anchor + "pub(crate) mod benchmark;\n", 1), encoding="utf-8")

    main = build / "src" / "main.rs"
    source = main.read_text(encoding="utf-8")
    anchor = "fn main() {"
    if source.count(anchor) != 1:
        raise SystemExit("could not locate the unique application entry point")
    branch = (
        "fn main() {\n"
        f'    if std::env::var_os("{MARKER}").is_some() {{\n'
        "        session::benchmark::run();\n"
        "        return;\n"
        "    }"
    )
    main.write_text(source.replace(anchor, branch, 1), encoding="utf-8")
    shutil.copy2(DRIVER, build / "src" / "session" / "benchmark.rs")

    source_hash, source_files = compile_input_digest(REPO)
    copied_hash, copied_files = tree_digest(build)
    manifest = {
        "format": 1,
        "method": "temporary copied crate; ordinary release build; cfg(test)=false; system allocator",
        "source_root": str(REPO),
        "source_tree_sha256": source_hash,
        "source_files": source_files,
        "prepared_tree_sha256": copied_hash,
        "prepared_files": copied_files,
        "driver_sha256": digest(DRIVER),
        "injected_files": ["src/main.rs", "src/session.rs", "src/session/benchmark.rs"],
        "build_command": "cargo build --locked --release --bin vivac",
    }
    (build / "opening-phase-manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps({"build_dir": str(build), "source_tree_sha256": source_hash,
                      "prepared_tree_sha256": copied_hash, "driver_sha256": manifest["driver_sha256"]}))


def run(args: argparse.Namespace) -> None:
    build = Path(args.build_dir).expanduser().resolve()
    manifest_path = build / "opening-phase-manifest.json"
    binary = build / "target" / "release" / ("vivac.exe" if os.name == "nt" else "vivac")
    if not manifest_path.is_file() or not binary.is_file():
        raise SystemExit("prepared source or release binary is missing")
    age = time.time() - binary.stat().st_mtime
    if age < MIN_BINARY_AGE_SECONDS:
        raise SystemExit(f"release binary must be at least {MIN_BINARY_AGE_SECONDS}s old; current age={age:.1f}s")
    fixture = Path(args.fixture_root).expanduser().resolve()
    if fixture == REPO or REPO in fixture.parents or fixture in REPO.parents:
        raise SystemExit("fixture-root must be outside the live vivac checkout")
    for parent in (fixture, *fixture.parents):
        if (parent / ".vivac").exists():
            raise SystemExit("fixture-root cannot be beneath a live .vivac tree")
    fixture.mkdir(parents=True, exist_ok=True)
    if any(fixture.iterdir()):
        raise SystemExit("fixture-root must be empty and dedicated to this run")
    output = Path(args.output).expanduser().resolve()
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    expected_copy_hash = manifest["prepared_tree_sha256"]
    copied_hash, _ = tree_digest(build, include_manifest=False)
    if copied_hash != expected_copy_hash:
        raise SystemExit("prepared source copy changed after its manifest was written")
    source_hash, source_files = compile_input_digest(REPO)
    copied_driver = build / "src" / "session" / "benchmark.rs"
    if digest(copied_driver) != manifest["driver_sha256"]:
        raise SystemExit("copied benchmark driver differs from its recorded source")
    before_binary_hash = digest(binary)
    run_rows = []
    for index_mode in ("missing", "warm"):
        for receipt_mode in ("stable", "refresh"):
            case_root = fixture / f"{index_mode}-{receipt_mode}"
            env = os.environ.copy()
            env[MARKER] = "1"
            env["VIVAC_OPENING_ROOT"] = str(case_root)
            env["VIVAC_OPENING_INDEX"] = index_mode
            env["VIVAC_OPENING_RECEIPT"] = receipt_mode
            env["VIVAC_OPENING_NODES"] = str(args.nodes)
            env["VIVAC_OPENING_FRONTS"] = str(args.fronts)
            env["VIVAC_OPENING_SAMPLES"] = str(args.samples)
            env["VIVAC_OPENING_WARMUPS"] = str(args.warmups)
            env["VIVAC_OPENING_OUTPUT"] = str(fixture / f"{index_mode}-{receipt_mode}.raw.json")
            env["VIVAC_HOME"] = str(case_root / "isolated-home")
            env["NO_COLOR"] = "1"
            completed = subprocess.run(
                [str(binary)], env=env, cwd=build, text=True, capture_output=True, check=False
            )
            if completed.returncode:
                raise SystemExit(
                    f"benchmark failed ({index_mode}/{receipt_mode}, exit {completed.returncode}):\n"
                    f"{completed.stderr[-4000:]}"
                )
            raw_path = Path(env["VIVAC_OPENING_OUTPUT"])
            run_rows.append(json.loads(raw_path.read_text(encoding="utf-8")))
            copied_hash, _ = tree_digest(build, include_manifest=False)
            if copied_hash != expected_copy_hash:
                raise SystemExit("prepared source copy changed while the benchmark was running")
            if digest(binary) != before_binary_hash:
                raise SystemExit("benchmark executable changed while measurements were running")
    final = {
        "format": 1,
        "method": "normal System allocator; release; startup excluded; phase timers use Instant; nearest-rank p50/p99",
        "binary": str(binary),
        "binary_sha256": before_binary_hash,
        "binary_mtime_epoch": binary.stat().st_mtime,
        "binary_age_seconds": age,
        "prepared_tree_sha256": expected_copy_hash,
        "source_tree_sha256_at_run": source_hash,
        "source_files_at_run": source_files,
        "filesystem": args.filesystem,
        "rust_build": args.rust_build,
        "fixture_root": str(fixture),
        "nodes": args.nodes,
        "fronts": args.fronts,
        "samples_per_case": args.samples,
        "warmups_per_case": args.warmups,
        "cases": run_rows,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(final, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(output), "binary_sha256": final["binary_sha256"],
                      "cases": len(run_rows), "samples_per_case": args.samples}))


def repair_driver(args: argparse.Namespace) -> None:
    build = Path(args.build_dir).expanduser().resolve()
    manifest_path = build / "opening-phase-manifest.json"
    binary = build / "target" / "release" / ("vivac.exe" if os.name == "nt" else "vivac")
    if binary.exists():
        raise SystemExit("refusing repair after the release executable exists")
    if not manifest_path.is_file():
        raise SystemExit("prepared source manifest is missing")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    current_hash, _ = tree_digest(build, include_manifest=False)
    if current_hash != manifest["prepared_tree_sha256"]:
        raise SystemExit("prepared source copy drifted; refusing to repair an unknown build")
    copied_driver = build / "src" / "session" / "benchmark.rs"
    shutil.copy2(DRIVER, copied_driver)
    manifest["driver_sha256"] = digest(copied_driver)
    manifest["prepared_tree_sha256"], manifest["prepared_files"] = tree_digest(
        build, include_manifest=False
    )
    manifest.setdefault("repair_history", []).append({
        "reason": args.reason,
        "repair": "replaced injected benchmark driver from tools/opening-phases.rs before a successful binary existed",
        "driver_sha256": manifest["driver_sha256"],
    })
    manifest["method"] = (
        "temporary copied crate; ordinary release build; cfg(test)=false; system allocator; "
        "driver repair recorded in repair_history"
    )
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "build_dir": str(build),
        "source_tree_sha256_preserved": manifest["source_tree_sha256"],
        "prepared_tree_sha256": manifest["prepared_tree_sha256"],
        "driver_sha256": manifest["driver_sha256"],
        "repair_count": len(manifest["repair_history"]),
    }))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prep = commands.add_parser("prepare", help="copy and inject the temporary driver")
    prep.add_argument("--build-dir", required=True)
    prep.set_defaults(func=prepare)
    repair = commands.add_parser("repair-driver", help="repair the injected driver before a binary exists")
    repair.add_argument("--build-dir", required=True)
    repair.add_argument("--reason", required=True)
    repair.set_defaults(func=repair_driver)
    execute = commands.add_parser("run", help="measure an aged coordinated release binary")
    execute.add_argument("--build-dir", required=True)
    execute.add_argument("--fixture-root", required=True)
    execute.add_argument("--output", required=True)
    execute.add_argument("--nodes", type=int, default=10_000)
    execute.add_argument("--fronts", type=int, default=100)
    execute.add_argument("--samples", type=int, default=200)
    execute.add_argument("--warmups", type=int, default=5)
    execute.add_argument("--filesystem", required=True)
    execute.add_argument("--rust-build", required=True,
                         help="declared rustc/toolchain/profile used for the coordinated release build")
    execute.set_defaults(func=run)
    args = parser.parse_args()
    if getattr(args, "samples", 1) > 200 or getattr(args, "samples", 1) < 1:
        parser.error("samples must be between 1 and 200 per case")
    if getattr(args, "warmups", 0) < 0:
        parser.error("warmups cannot be negative")
    if args.command == "run" and (args.nodes < args.fronts or args.fronts < 1):
        parser.error("nodes must be >= fronts >= 1")
    args.func(args)


if __name__ == "__main__":
    main()
