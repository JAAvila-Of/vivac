#!/usr/bin/env python3
"""Disposable requested-heap observations for the package checker."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import struct
import sys
import tempfile

import lifecycle_probe as life
import openssh_probe as native
import provider_probe as provider_tools
import session_probe as session


BUDGET = 2 * 1024 * 1024
FIXTURE_READ_LIMIT = 65537
SAMPLES = 3
MEASUREMENT = "additional-live-rust-requested-heap"
CASES = {"valid-package": True, "rounds-u32max": False, "key-count-u32max": False,
         "declared-body-1048575": False, "declared-body-1048576": False,
         "declared-body-4294967295": False, "declared-salt-1048575": False,
         "near-cap-large-field": False, "file-65536-malformed": False,
         "file-65537-oversize": False}
FIELDS = {"version", "measurement", "accepted", "baseline_bytes", "peak_additional_bytes",
          "live_after_bytes", "budget_bytes", "within_budget", "allocator_valid", "realloc_conservative"}
CURRENT_REPORT = None
CURRENT_PHASE = "preflight"
UNCERTAIN_BUNDLES = []


class ProbeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def safe_error(error, phase):
    return provider_tools.safe_error(error, phase)


def file_sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, "heap metadata has duplicate fields")
        value[key] = item
    return value


def parse_observation(output):
    require(isinstance(output, bytes) and 0 < len(output) <= 4096, "heap metadata size was refused")
    try:
        value = json.loads(output.decode("ascii"), object_pairs_hook=unique_object)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise ProbeError("heap metadata encoding was refused") from error
    require(isinstance(value, dict) and set(value) == FIELDS, "heap metadata schema was refused")
    require(type(value["version"]) is int and value["version"] == 1
            and value["measurement"] == MEASUREMENT, "heap measurement kind was refused")
    for name in ("accepted", "within_budget", "allocator_valid", "realloc_conservative"):
        require(type(value[name]) is bool, "heap metadata boolean was refused")
    for name in ("baseline_bytes", "peak_additional_bytes", "live_after_bytes", "budget_bytes"):
        require(type(value[name]) is int and 0 <= value[name] <= 0xffffffffffffffff,
                "heap metadata counter was refused")
    return value


def validate_observation(value, expected, code):
    require(type(expected) is bool and type(code) is int, "heap control type was refused")
    require(value["budget_bytes"] == BUDGET and value["allocator_valid"] is True
            and value["realloc_conservative"] is True, "heap allocator contract was not established")
    require(value["accepted"] is expected, "instrumented checker differs from its control")
    peak, baseline = value["peak_additional_bytes"], value["baseline_bytes"]
    require(peak <= 0xffffffffffffffff - baseline and value["live_after_bytes"] <= baseline + peak,
            "heap counters are inconsistent")
    require(code == 0 and value["within_budget"] is True and peak <= BUDGET,
            "requested heap exceeds its hard ceiling or measurement failed")
    return value


def header(decoded):
    reader = native.Reader(decoded)
    require(reader.take(15) == b"openssh-key-v1\0", "fixture key magic was refused")
    fields = [reader.string(1024) for _ in range(3)]
    prefix = b"openssh-key-v1\0" + b"".join(native.wire_string(field) for field in fields)
    count = reader.number()
    rest = decoded[reader.position:]
    public = reader.string(128)
    body_prefix = prefix + struct.pack(">I", count) + native.wire_string(public)
    return prefix, rest, body_prefix


def mutated_key(decoded, name):
    package = session.package_backend()
    require(name in CASES and name != "valid-package", "heap fixture case was refused")
    if name == "file-65536-malformed":
        return b"x" * 65536
    if name == "file-65537-oversize":
        return b"x" * 65537
    if name == "rounds-u32max":
        options = native.wire_string(b"s" * 16) + struct.pack(">I", 0xffffffff)
        return package.encode_native(package.replace_key_field(decoded, 2, options))
    if name == "declared-salt-1048575":
        options = struct.pack(">I", 1048575) + b"s" * 256 + struct.pack(">I", 64)
        return package.encode_native(package.replace_key_field(decoded, 2, options))
    prefix, rest, body_prefix = header(decoded)
    if name == "key-count-u32max":
        return package.encode_native(prefix + struct.pack(">I", 0xffffffff) + rest)
    if name.startswith("declared-body-"):
        size = int(name.removeprefix("declared-body-"))
        require(size in (1048575, 1048576, 0xffffffff), "heap field length was refused")
        return package.encode_native(body_prefix + struct.pack(">I", size) + b"x" * 256)
    require(name == "near-cap-large-field", "heap fixture mutation was refused")
    declared = body_prefix + struct.pack(">I", 1048575)
    low, high = 0, 49152
    while low < high:
        middle = (low + high + 1) // 2
        if len(package.encode_native(declared + b"x" * middle)) <= native.KEY_LIMIT:
            low = middle
        else:
            high = middle - 1
    result = package.encode_native(declared + b"x" * low)
    require(65000 <= len(result) <= native.KEY_LIMIT, "near-cap fixture did not reach its selected boundary")
    return result


def fixture_identity(value):
    return tuple(getattr(value, name) for name in
                 ("st_dev", "st_ino", "st_mode", "st_uid", "st_gid", "st_nlink",
                  "st_size", "st_mtime_ns", "st_ctime_ns"))


def verify_fixture_bytes(path, expected, *, limit=FIXTURE_READ_LIMIT):
    # Fixture publication includes one intentionally oversized rejection input.
    # The product reader and its 64 KiB admission ceiling remain unchanged.
    require(limit in (2048, FIXTURE_READ_LIMIT), "fixture readback bound was refused")
    require(isinstance(expected, bytes) and 0 < len(expected) <= limit,
            "fixture readback length was refused")
    before = path.lstat()
    require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1
            and before.st_uid == os.geteuid() and not before.st_mode & 0o077
            and before.st_size == len(expected), "fixture readback metadata was refused")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as source:
        require(fixture_identity(os.fstat(source.fileno())) == fixture_identity(before),
                "fixture readback object changed during opening")
        actual = source.read(len(expected))
        require(actual == expected, "fixture readback contents differ")
        require(fixture_identity(os.fstat(source.fileno())) == fixture_identity(before)
                == fixture_identity(path.lstat()), "fixture readback object changed during reading")


def create_variant(base, destination, name):
    package = session.package_backend()
    manifest, _ = package.read_manifest(base)
    decoded = native.parse_key(base / "key.openssh")["decoded"]
    contents = mutated_key(decoded, name)
    changed = package.update_manifest_for_key(manifest, contents)
    encoded_manifest = package.manifest_bytes(changed)
    require(0 < len(encoded_manifest) <= 2048, "fixture manifest exceeded its publication bound")
    destination.mkdir(mode=0o700)
    native.private_write(destination / "key.openssh", contents)
    native.private_write(destination / "manifest.json", encoded_manifest)
    verify_fixture_bytes(destination / "manifest.json", encoded_manifest, limit=2048)
    verify_fixture_bytes(destination / "key.openssh", contents)
    return destination


def quiescent_call(bundle, operation):
    # A raised child operation does not prove that its process exit was observed.
    bundle["process_exit_uncertain"] = True
    result = operation()
    bundle["process_exit_uncertain"] = False
    return result


def observe(bundle, helper, path, env):
    code, output = quiescent_call(bundle, lambda: native.bounded_run(
        [str(helper), "--heap-package-check", str(path)], env))
    return code, parse_observation(output)


def collect_case(report, bundle, name, writer, helper, path, env):
    expected = CASES[name]
    case = {"case": name, "expected_accepted": expected, "observations": [],
            "mutation_rehashed": name != "valid-package"}
    report["cases"].append(case)
    package = session.package_backend()
    code, _ = quiescent_call(bundle, lambda: package.check(writer, path, env))
    case["baseline_checker_exit_code"] = code
    require(code == (0 if expected else 1), "baseline checker differs from selected fixture outcome")
    for _ in range(SAMPLES):
        code, value = observe(bundle, helper, path, env)
        case["observations"].append(value)
        case["peak_additional_bytes"] = max(item["peak_additional_bytes"] for item in case["observations"])
        validate_observation(value, expected, code)
    return case


def validate_case(value):
    name = value.get("case")
    require(name in CASES and value.get("expected_accepted") is CASES[name], "heap case identity was refused")
    expected = CASES[name]
    require(type(value.get("baseline_checker_exit_code")) is int
            and value["baseline_checker_exit_code"] == (0 if expected else 1),
            "heap case baseline control differs")
    require(value.get("mutation_rehashed") is (name != "valid-package"), "heap fixture rehash was not established")
    observations = value.get("observations")
    require(isinstance(observations, list) and len(observations) == SAMPLES, "heap case is incomplete")
    for item in observations:
        validate_observation(item, expected, 0)
    require(type(value.get("peak_additional_bytes")) is int
            and value["peak_additional_bytes"] == max(item["peak_additional_bytes"] for item in observations),
            "heap case summary differs from raw observations")
    return True


def retain_control(report, bundle, writer, helper, path, env):
    package = session.package_backend()
    require(quiescent_call(bundle, lambda: package.check(writer, path, env))[0] == 0,
            "retained control package was refused")
    code, control = observe(bundle, helper, path, env)
    report["retained_control_observation"] = control
    validate_observation(control, True, code)
    report["retained_control_accepted"] = True


def finish(bundle, primary):
    if primary is not None and CURRENT_REPORT is not None:
        CURRENT_REPORT["primary_failure"] = safe_error(primary, CURRENT_PHASE)
    if bundle.get("process_exit_uncertain"):
        UNCERTAIN_BUNDLES.append(bundle)
        error = ProbeError("checker process exit remains uncertain")
        if CURRENT_REPORT is not None:
            CURRENT_REPORT["cleanup_failure"] = safe_error(error, "cleanup-native-exit")
        if primary is None:
            raise error
        return
    try:
        bundle["provider"].close()
        package = session.package_backend()
        for root, identity in bundle["roots"]:
            package.cleanup_owned_tree(root, identity)
    except BaseException as error:
        UNCERTAIN_BUNDLES.append(bundle)
        if CURRENT_REPORT is not None:
            CURRENT_REPORT["cleanup_failure"] = safe_error(error, "cleanup-resources")
        if primary is None:
            raise


def run_probe(writer, verifier, heap_helper, store):
    global CURRENT_REPORT, CURRENT_PHASE
    report = {"version": 1, "status": "incomplete", "custody_accredited": False,
              "kdf_policy_adopted": False, "performance_admitted": False,
              "heap_measurements_tested": False, "parser_auxiliary_heap_measured": False,
              "measurement_kind": MEASUREMENT, "budget_bytes": BUDGET, "samples_per_case": SAMPLES,
              "control_kind": "valid-package-check", "retained_control_accepted": False, "cases": []}
    CURRENT_REPORT = report
    life.require_isolation()
    life.require_unprivileged()
    require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer is missing no_new_privs")
    report["resource_limits"] = life.require_resources(expected_script="heap_probe.py")
    session.require_store(store)
    package = session.package_backend()
    root = Path(tempfile.mkdtemp(prefix="vivac-heap-", dir="/tmp"))
    root.chmod(0o700)
    store_root = store / "heap-fixtures"
    store_root.mkdir(mode=0o700)
    roots = [(root, package.validate_safe_tree_root(root)),
             (store_root, package.validate_safe_tree_root(store_root))]
    provider = provider_tools.Provider()
    bundle = {"provider": provider, "roots": roots, "process_exit_uncertain": False}
    primary = None
    try:
        CURRENT_PHASE = "provider-admission"
        (root / "home").mkdir(mode=0o700)
        env = life.private_env(root)
        provider.open()
        version, architecture = quiescent_call(bundle, lambda: provider.package(env))
        report["provenance"] = {"os_id": "ubuntu", "os_version": "24.04", "package_name": "openssh-client",
                                "package_version": version, "package_arch": architecture,
                                "components": provider.components()}
        report["helper_sha256"] = {"writer": file_sha256(writer), "verifier": file_sha256(verifier),
                                   "heap_helper": file_sha256(heap_helper)}
        report["source_sha256"] = {"heap_probe": file_sha256(Path(__file__).resolve()),
                                   "provider_probe": file_sha256(Path(provider_tools.__file__).resolve()),
                                   "package_probe": file_sha256(Path(package.__file__).resolve())}
        CURRENT_PHASE = "control-package"
        baseline = store_root / "control"
        quiescent_call(bundle, lambda: package.create_positive(writer, baseline, env))
        require(quiescent_call(bundle, lambda: package.check(writer, baseline, env))[0] == 0,
                "baseline package was refused")
        report["baseline_input_bytes"] = (baseline / "key.openssh").stat().st_size
        for name in CASES:
            CURRENT_PHASE = name
            path = baseline if name == "valid-package" else create_variant(baseline, store_root / name, name)
            collect_case(report, bundle, name, writer, heap_helper, path, env)
        CURRENT_PHASE = "retained-control"
        retain_control(report, bundle, writer, heap_helper, baseline, env)
        provider.revalidate()
        report["provenance"]["components"] = provider.components()
        require(len(report["cases"]) == len(CASES) and {item["case"] for item in report["cases"]} == set(CASES)
                and all(validate_case(item) for item in report["cases"]),
                "heap measurement matrix is incomplete")
        report.update(all_measurements_valid=True, fixtures_rehashed=True, checker_equivalence=True)
    except BaseException as error:
        primary = error
        raise
    finally:
        finish(bundle, primary)
    report.update(resources_closed_after_measurement=True, heap_measurements_tested=True,
                  parser_auxiliary_heap_measured=True, status="observed")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("writer", "verifier", "heap-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "this experiment requires an explicitly disposable Linux container")
        session.require_report_directory(args.report)
        result = run_probe(args.writer.resolve(strict=True), args.verifier.resolve(strict=True),
                           args.heap_helper.resolve(strict=True), args.store_root)
        life.private_write(args.report, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("Requested-heap observations recorded; policy_adopted=false")
        return 0
    except Exception as error:
        try:
            session.require_report_directory(args.report)
            result = dict(CURRENT_REPORT or {})
            result.update(version=1, status="incomplete", custody_accredited=False,
                          phase=CURRENT_PHASE, failure=safe_error(error, CURRENT_PHASE))
            life.private_write(args.report, json.dumps(result, sort_keys=True).encode("ascii") + b"\n")
        except (OSError, session.ProbeError):
            pass
        print("Requested-heap probe failed; no policy is admitted.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
