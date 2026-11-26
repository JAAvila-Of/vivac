#!/usr/bin/env python3
"""Disposable wall-time measurements of the fixed-round encrypted-key profile."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import sys
import tempfile
import time

import lifecycle_probe as life
import openssh_probe as native
import provider_probe as provider_tools
import session_probe as session


SAMPLES = 7
ROUNDS = (0, 1, 16, 63, 65, 4294967295)
CASES = ("package-create", "activation-proof", "unlocked-sign-proof",
         *("reject-rounds-" + str(value) for value in ROUNDS), "reject-large-field")
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


def checked_dispatch(writer, path, env, callback):
    code, _ = session.package_backend().check(writer, path, env)
    if code != 0:
        return code
    callback()
    return 0


def sample_case(name, samples):
    require(name in CASES and len(samples) == SAMPLES, "measurement case or sample count was refused")
    require(all(type(value) in (int, float) and math.isfinite(value) and value >= 0 for value in samples),
            "measurement sample was refused")
    nominal = 2000 if name in ("package-create", "activation-proof") else 50
    tolerance = nominal * 11 / 10
    peak = max(samples)
    classification = "nominal" if peak <= nominal else "within-tolerance" if peak <= tolerance else "above-band"
    return {"case": name, "samples_ms": list(samples), "nominal_ms": nominal,
            "tolerance_ms": tolerance, "classification": classification}


def record_attempt(start, returned):
    elapsed = (time.perf_counter_ns() - start) / 1_000_000
    require(math.isfinite(elapsed) and elapsed >= 0, "measurement clock was refused")
    if CURRENT_REPORT is not None:
        CURRENT_REPORT.setdefault("raw_attempts", []).append(
            {"case": CURRENT_PHASE, "elapsed_ms": elapsed, "returned": returned})
    return elapsed


def timed(operation):
    start = time.perf_counter_ns()
    try:
        result = operation()
    except BaseException:
        try:
            record_attempt(start, False)
        except BaseException as error:
            if CURRENT_REPORT is not None:
                CURRENT_REPORT["measurement_failure"] = safe_error(error, CURRENT_PHASE)
        raise
    return record_attempt(start, True), result


def mutated_key(decoded, name):
    package = session.package_backend()
    if name.startswith("reject-rounds-"):
        value = int(name.removeprefix("reject-rounds-"))
        require(value in ROUNDS, "round mutation is outside the fixture")
        options = native.wire_string(b"s" * 16) + struct.pack(">I", value)
        return package.encode_native(package.replace_key_field(decoded, 2, options))
    require(name == "reject-large-field", "key mutation is outside the fixture")
    reader = native.Reader(decoded)
    require(reader.take(15) == b"openssh-key-v1\0", "fixture key magic was refused")
    fields = [reader.string(1024) for _ in range(3)]
    count, public_blob = reader.number(), reader.string(128)
    prefix = (b"openssh-key-v1\0" + b"".join(native.wire_string(value) for value in fields)
              + struct.pack(">I", count) + native.wire_string(public_blob)
              + struct.pack(">I", 1024 * 1024) + b"x" * 256)
    result = package.encode_native(prefix)
    require(len(result) <= native.KEY_LIMIT, "malformed fixture exceeded its input bound")
    return result


def create_variant(base, destination, name):
    package = session.package_backend()
    manifest, _ = package.read_manifest(base)
    parsed = native.parse_key(base / "key.openssh")
    contents = mutated_key(parsed["decoded"], name)
    changed = package.update_manifest_for_key(manifest, contents)
    destination.mkdir(mode=0o700)
    native.private_write(destination / "key.openssh", contents)
    native.private_write(destination / "manifest.json", package.manifest_bytes(changed))
    reread, _ = package.read_manifest(destination)
    require(reread == changed and native.read_private_file(destination / "key.openssh") == contents,
            "mutated package changed during publication")
    return destination


def validate_created(writer, destination, env, device, credential):
    package = session.package_backend()
    require(package.check(writer, destination, env)[0] == 0, "created package was refused")
    snapshot = package.open_retained_package(destination)
    try:
        package.revalidate_retained_package(snapshot)
        parsed = native.parse_key_bytes(snapshot["key_bytes"])
        manifest = snapshot["manifest"]
        require(parsed["cipher"] == "aes256-gcm@openssh.com" and parsed["kdf"] == "bcrypt"
                and parsed["rounds"] == 64 and manifest["device_id"] == device
                and manifest["credential_id"] == credential
                and manifest["public_key"] == parsed["public_key"].hex(),
                "created package identity or profile differs")
        package.revalidate_retained_package(snapshot)
    finally:
        package.close_retained_package(snapshot)


class Credential:
    def __init__(self, provider, verifier, package_path, root, name):
        self.provider, self.verifier, self.package_path, self.root = provider, verifier, package_path, root
        self.agent = provider_tools.OwnedAgent(provider, root, name)
        self.env = life.private_env(root, self.agent.path)
        self.snapshot = self.memfd = self.parsed = None
        self.proofs_verified = 0
        self.proof_sequence = 0

    def validate(self):
        require(self.snapshot is not None, "credential revision is not retained")
        session.package_backend().revalidate_retained_package(self.snapshot)
        self.provider.revalidate()
        if self.agent.process is not None:
            require(self.agent.pidfd is not None and not life.pidfd_ready(self.agent.pidfd, 0),
                    "owned signing agent is not alive")

    def activate(self):
        package = session.package_backend()
        self.snapshot = package.open_retained_package(self.package_path)
        self.validate()
        self.parsed = native.parse_key_bytes(self.snapshot["key_bytes"])
        require(self.parsed["cipher"] == "aes256-gcm@openssh.com" and self.parsed["kdf"] == "bcrypt"
                and self.parsed["rounds"] == 64, "activation profile was refused")
        self.memfd, _ = package.make_sealed_key_memfd(self.snapshot["key_bytes"])
        self.agent.start()
        self.validate()
        ciphertext = "/proc/" + str(os.getpid()) + "/fd/" + str(self.memfd)
        code, _ = self.provider.execute("ssh-add", ["-k", ciphertext], self.env, responses=[native.PASSWORD])
        self.validate()
        require(code == 0, "native activation refused the sealed ciphertext")
        manifest = self.snapshot["manifest"]
        comment = "vivac.device-key.v1:" + manifest["device_id"] + ":" + manifest["credential_id"]
        require(native.identities(self.agent.path) == [(self.parsed["public_blob"], comment.encode("ascii"))],
                "native loaded identity differs")
        self.sign_proof()

    def sign_proof(self):
        self.validate()
        manifest = self.snapshot["manifest"]
        challenge = native.challenge(self.parsed["public_key"], manifest["device_id"], manifest["credential_id"])
        signature = native.sign(self.agent.path, self.parsed["public_blob"], native.canonical(challenge))
        self.proof_sequence += 1
        name = self.agent.path.stem + "-proof-" + str(self.proof_sequence)
        proof_path = self.root / (name + ".json")
        try:
            native.verify(self.verifier, self.root, self.env, native.proof(challenge, signature), name, True)
            self.validate()
            self.proofs_verified += 1
        finally:
            if proof_path.exists():
                proof_path.unlink()

    def stop(self):
        self.agent.stop()

    def release(self):
        require(self.agent.process is None or self.agent.exit_observed,
                "credential resources precede native exit")
        self.agent.close()
        if self.memfd is not None:
            descriptor, self.memfd = self.memfd, None
            os.close(descriptor)
        if self.snapshot is not None:
            snapshot, self.snapshot = self.snapshot, None
            session.package_backend().close_retained_package(snapshot)


def finish(bundle, primary):
    if primary is not None and CURRENT_REPORT is not None:
        CURRENT_REPORT["primary_failure"] = safe_error(primary, CURRENT_PHASE)
    errors = []
    for credential in bundle["credentials"]:
        try:
            credential.stop()
        except BaseException as error:
            errors.append(error)
    if errors:
        UNCERTAIN_BUNDLES.append(bundle)
        if CURRENT_REPORT is not None:
            CURRENT_REPORT["cleanup_failure"] = safe_error(errors[0], "cleanup-native-exit")
        if primary is None:
            raise ProbeError("native shutdown remains uncertain") from errors[0]
        return
    try:
        for credential in bundle["credentials"]:
            credential.release()
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


def run_probe(writer, verifier, store, series):
    import resource

    global CURRENT_REPORT, CURRENT_PHASE
    require(series in ("A", "B"), "measurement series was refused")
    report = {"version": 1, "status": "incomplete", "custody_accredited": False,
              "kdf_policy_adopted": False, "performance_admitted": False,
              "parser_auxiliary_heap_measured": False, "p99_estimated": False,
              "kdf_measurements_tested": False, "series": series, "samples_per_case": SAMPLES,
              "rounds": 64, "cases": [], "raw_attempts": [], "sentinel_survived": False}
    CURRENT_REPORT = report
    life.require_isolation()
    life.require_unprivileged()
    require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer is missing no_new_privs")
    report["resource_limits"] = life.require_resources(expected_script="kdf_probe.py")
    session.require_store(store)
    package = session.package_backend()
    root = Path(tempfile.mkdtemp(prefix="vivac-kdf-", dir="/tmp"))
    root.chmod(0o700)
    store_root = store / ("series-" + series)
    store_root.mkdir(mode=0o700)
    roots = [(root, package.validate_safe_tree_root(root)),
             (store_root, package.validate_safe_tree_root(store_root))]
    provider = provider_tools.Provider()
    bundle = {"credentials": [], "provider": provider, "roots": roots}
    primary = None
    try:
        CURRENT_PHASE = "provider-admission"
        (root / "home").mkdir(mode=0o700)
        env = life.private_env(root)
        provider.open()
        version, architecture = provider.package(env)
        report["provenance"] = {"os_id": "ubuntu", "os_version": "24.04",
                                "package_name": "openssh-client", "package_version": version,
                                "package_arch": architecture, "components": provider.components()}
        report["helper_sha256"] = {"writer": file_sha256(writer), "verifier": file_sha256(verifier)}
        report["source_sha256"] = {"kdf_probe": file_sha256(Path(__file__).resolve()),
                                   "provider_probe": file_sha256(Path(provider_tools.__file__).resolve()),
                                   "package_probe": file_sha256(Path(package.__file__).resolve())}
        CURRENT_PHASE = "control-package"
        baseline = store_root / "control"
        package.create_positive(writer, baseline, env)
        require(package.check(writer, baseline, env)[0] == 0, "control package was refused")

        def credential(name):
            value = Credential(provider, verifier, baseline, root, name)
            bundle["credentials"].append(value)
            return value

        sentinel = credential("sentinel")
        require(checked_dispatch(writer, baseline, env, sentinel.activate) == 0,
                "sentinel structural admission was refused")
        CURRENT_PHASE = "package-create"
        samples = []
        for index in range(SAMPLES):
            destination = store_root / ("created-" + str(index))
            device, identity = native.identifier(), native.identifier()
            comment = "vivac.device-key.v1:" + device + ":" + identity
            elapsed, result = timed(lambda: package.create(writer, destination, comment, env))
            require(result[0] == 0, "timed package creation failed")
            validate_created(writer, destination, env, device, identity)
            samples.append(elapsed)
        report["cases"].append(sample_case("package-create", samples))

        CURRENT_PHASE = "activation-proof"
        samples = []
        for index in range(SAMPLES):
            current = credential("activation-" + str(index))
            elapsed, code = timed(lambda: checked_dispatch(writer, baseline, env, current.activate))
            require(code == 0 and current.proofs_verified == 1, "activation proof was not verified")
            current.stop()
            current.release()
            samples.append(elapsed)
        report["cases"].append(sample_case("activation-proof", samples))

        CURRENT_PHASE = "unlocked-sign-proof"
        signer = credential("signer")
        require(checked_dispatch(writer, baseline, env, signer.activate) == 0,
                "signer structural admission was refused")
        samples = []
        for _ in range(SAMPLES):
            elapsed, _ = timed(signer.sign_proof)
            samples.append(elapsed)
        require(signer.proofs_verified == SAMPLES + 1, "unlocked proofs were not all verified")
        report["cases"].append(sample_case("unlocked-sign-proof", samples))

        for name in CASES[3:]:
            CURRENT_PHASE = name
            require(package.check(writer, baseline, env)[0] == 0, "negative control package was refused")
            variant = create_variant(baseline, store_root / name, name)
            dispatches = []

            def forbidden_dispatch():
                dispatches.append(True)
                raise ProbeError("negative fixture reached native dispatch")

            samples, codes = [], []
            for _ in range(SAMPLES):
                elapsed, code = timed(lambda: checked_dispatch(writer, variant, env, forbidden_dispatch))
                require(code == 1 and not dispatches, "structural refusal or dispatch boundary differs")
                samples.append(elapsed)
                codes.append(code)
            case = sample_case(name, samples)
            case.update(structural_exit_codes=codes, native_dispatches=len(dispatches),
                        control_package_accepted=True, mutation_rehashed=True)
            report["cases"].append(case)

        CURRENT_PHASE = "final-proof"
        sentinel.sign_proof()
        require(sentinel.proofs_verified == 2, "sentinel did not survive the measurement controls")
        report["sentinel_survived"] = True
        provider.revalidate()
        for current in (sentinel, signer):
            current.validate()
        report["provenance"]["components"] = provider.components()
        require(len(report["cases"]) == len(CASES) and {value["case"] for value in report["cases"]} == set(CASES),
                "measurement matrix is incomplete")
        report.update(all_proofs_verified=True, profile_refusals_before_native=True, snapshots_revalidated=True)
    except BaseException as error:
        primary = error
        raise
    finally:
        finish(bundle, primary)
    require(all(value.agent.exit_observed for value in bundle["credentials"]), "native exits were not observed")
    report.update(native_exits_observed=True, retained_resources_closed_after_exit=True,
                  child_maxrss_kib=resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss,
                  kdf_measurements_tested=True, status="observed")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("writer", "verifier", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--series", choices=("A", "B"), required=True)
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "this experiment requires an explicitly disposable Linux container")
        session.require_report_directory(args.report)
        result = run_probe(args.writer.resolve(strict=True), args.verifier.resolve(strict=True), args.store_root, args.series)
        life.private_write(args.report, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("KDF measurements recorded; policy_adopted=false")
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
        print("KDF measurement probe failed; no policy is admitted.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
