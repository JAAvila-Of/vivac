#!/usr/bin/env python3
"""Disposable native creation-binding observations with synthetic credentials."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time

import heap_probe as heap
import lifecycle_probe as life
import openssh_probe as native
import provider_probe as providers
import rust_launcher_probe as spawn
import rust_loader_probe as loader
import session_probe as session

CASES = ("complete", "wrong-key", "wrong-comment")
FLAGS = {"producer_origin_checked", "creation_admission", "custody_accredited"}
IDENTITY = ("agent_pid", "loader_pid", "creator_tid")
DOMAIN = b"vivac.local.creation-binding.v1\0"
DER_PREFIX = bytes.fromhex("302a300506032b6570032100")
OTHER_KEY = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
RESULT_FIELDS = {"stage", "case", *IDENTITY, "record_sha256", "identity_mismatch",
                 "native_binding_verified", "challenge", "signature", "ready_published", *FLAGS}
FINAL_FIELDS = RESULT_FIELDS | {"version", "status", "state", "active", "native_requests_sent",
    "native_replies_received", "proofs_verified", "proofs_published", "lock_callers",
    "lock_acknowledgments", "workers_joined", "close_acknowledged", "resources_closed_after_drain",
    "busy_refused", "closed_refused", "agent_exit_code", "agent_exit_signal", "loader_exit_code",
    "loader_exit_signal", "pressure_iterations"}
UNCERTAIN = []
PHASE = "preflight"
require = loader.require


def canonical(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=True).encode("ascii")


def decode(payload, fields):
    require(0 < len(payload) <= 8192, "creation packet size differs")
    value = json.loads(payload.decode("ascii"), object_pairs_hook=heap.unique_object)
    require(type(value) is dict and set(value) == fields, "creation packet fields differ")
    return value


def public_hex(value, size):
    require(type(value) is str and len(value) == size * 2
            and all(c in "0123456789abcdef" for c in value), "creation public encoding differs")
    return bytes.fromhex(value)


def make_record(snapshot, parsed, comment, case):
    require(case in CASES, "creation case differs")
    manifest = snapshot["manifest"]
    device, credential = manifest["device_id"], manifest["credential_id"]
    require(comment == "vivac.device-key.v1:" + device + ":" + credential
            and manifest["public_key"] == parsed["public_key"].hex(), "fixture identity differs")
    value = {"version": 2, "installation_id": "0" * 25 + "3", "creation_id": "0" * 25 + "4",
             "device_id": device, "credential_id": credential, "public_key": parsed["public_key"].hex(),
             "ciphertext_sha256": hashlib.sha256(snapshot["key_bytes"]).hexdigest(),
             "manifest_sha256": hashlib.sha256(snapshot["manifest_bytes"]).hexdigest(),
             "ciphertext_len": len(snapshot["key_bytes"]), "manifest_len": len(snapshot["manifest_bytes"]),
             "creation_profile_revision": 1, "secret_policy_revision": 1}
    if case == "wrong-key":
        require(value["public_key"] != OTHER_KEY, "replacement key equals fixture key")
        value["public_key"] = OTHER_KEY
    elif case == "wrong-comment":
        require(device != "0" * 25 + "5", "replacement comment equals fixture comment")
        value["device_id"] = "0" * 25 + "5"
    return value


def check_record_change(record, original, case):
    require(case in CASES, "creation case differs")
    changed = {key for key in original if record.get(key) != original[key]}
    require(set(record) == set(original) and changed == (
        {"public_key"} if case == "wrong-key" else {"device_id"} if case == "wrong-comment" else set()),
        "creation fixture changed outside selected mismatch")
    if case == "wrong-key":
        require(record["public_key"] == OTHER_KEY, "replacement creation key differs")
    elif case == "wrong-comment":
        require(record["device_id"] == "0" * 25 + "5", "replacement creation comment differs")


def parse_startup(payload, stage, identity=None):
    require(stage in ("prompt", "handoff"), "creation startup stage differs")
    value = decode(payload, loader.FIELDS | FLAGS)
    require(all(value[key] is False for key in FLAGS), "creation startup claims admission")
    basic = loader.parse_stage(canonical({key: value[key] for key in loader.FIELDS}), stage)
    require(all(basic[key] is False for key in ("ready_published", "identity_bound", "proof_verified",
                                               "loader_failed", "close_ack", "resource_release_after_exit")),
            "creation startup claims completed validation")
    require(all(basic[key] is None for key in ("agent_exit_code", "agent_exit_signal",
                                               "loader_exit_code", "loader_exit_signal")),
            "creation startup claims premature exit")
    require(len({basic[key] for key in IDENTITY}) == 3, "creation process identities overlap")
    if identity is not None:
        require(all(basic[key] == identity[key] for key in IDENTITY), "creation ownership changed")
    return basic


def parse_result(payload, case, identity, record, final=False):
    require(case in CASES, "creation case differs")
    value = decode(payload, FINAL_FIELDS if final else RESULT_FIELDS)
    require(value["stage"] == ("creation-final" if final else "creation-result")
            and value["case"] == case, "creation result identity differs")
    for key in IDENTITY:
        require(type(value[key]) is int and value[key] > 0 and value[key] == identity[key],
                "creation native identity changed")
    require(len({value[key] for key in IDENTITY}) == 3, "creation process identities overlap")
    require(all(value[key] is False for key in FLAGS | {"ready_published"}), "creation result claims admission")
    expected_digest = hashlib.sha256(canonical(record)).digest()
    require(public_hex(value["record_sha256"], 32) == expected_digest, "creation record digest differs")
    complete = case == "complete"
    require(value["identity_mismatch"] is (not complete) and value["native_binding_verified"] is complete,
            "creation mismatch outcome differs")
    if complete:
        challenge = public_hex(value["challenge"], len(DOMAIN) + 64)
        require(challenge[:len(DOMAIN)] == DOMAIN and challenge[len(DOMAIN):len(DOMAIN) + 32] == expected_digest,
                "creation challenge purpose or record differs")
        public_hex(value["signature"], 64)
    else:
        require(value["challenge"] is None and value["signature"] is None, "negative case published proof")
    if final:
        require(type(value["version"]) is int and value["version"] == 1
                and value["status"] == "observed" and value["state"] == "CLOSED", "creation final state differs")
        for key in ("native_requests_sent", "native_replies_received", "proofs_verified", "proofs_published", "workers_joined"):
            require(type(value[key]) is int and value[key] == int(complete), "creation proof count differs")
        for key, expected in (("lock_callers", 1), ("lock_acknowledgments", 1), ("busy_refused", 0),
                              ("closed_refused", 0), ("agent_exit_signal", signal.SIGKILL), ("loader_exit_code", 0)):
            require(type(value[key]) is int and value[key] == expected, "creation close count or exit differs")
        require(value["agent_exit_code"] is None and value["loader_exit_signal"] is None,
                "creation native exit cause differs")
        require(type(value["pressure_iterations"]) is int and value["pressure_iterations"] > 0,
                "creation pressure observation differs")
        require(value["active"] is False and value["close_acknowledged"] is True
                and value["resources_closed_after_drain"] is True, "creation closure evidence differs")
    return value


def openssl_verify(verifier, root, env, public_key, message, signature, label):
    require(len(public_key) == 32 and len(signature) == 64 and message, "public verification input differs")
    paths = [root / (label + suffix) for suffix in (".der", ".message", ".signature", ".altered")]
    altered = bytes([message[0] ^ 1]) + message[1:]
    for path, content in zip(paths, (DER_PREFIX + public_key, message, signature, altered)):
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        try:
            require(os.write(descriptor, content) == len(content), "public verification file write differs")
        finally:
            os.close(descriptor)
    for index, expected in ((1, 0), (3, 1)):
        verifier.revalidate()
        command = [str(verifier.original), "pkeyutl", "-verify", "-pubin", "-inkey", str(paths[0]),
                   "-keyform", "DER", "-rawin", "-in", str(paths[index]), "-sigfile", str(paths[2])]
        process = subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, close_fds=True)
        try:
            deadline = time.monotonic() + 3
            count = 0
            with selectors.DefaultSelector() as selector:
                for stream in (process.stdout, process.stderr):
                    os.set_blocking(stream.fileno(), False)
                    selector.register(stream, selectors.EVENT_READ)
                while selector.get_map():
                    require(time.monotonic() < deadline, "OpenSSL verification deadline expired")
                    for key, _ in selector.select(0.02):
                        chunk = os.read(key.fd, 4096)
                        if not chunk:
                            selector.unregister(key.fileobj)
                        count += len(chunk)
                        require(count <= 16384, "OpenSSL verification output exceeded its bound")
            require(process.wait(timeout=max(0.001, deadline - time.monotonic())) == expected,
                    "OpenSSL verification or altered-message rejection differs")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)
            process.stdout.close()
            process.stderr.close()
        verifier.revalidate()


def run_case(args, provider, verifier, root, case, state):
    global PHASE
    PHASE = "fixture"
    case_root = root / case
    case_root.mkdir(mode=0o700)
    (case_root / "home").mkdir(mode=0o700)
    env = life.private_env(case_root)
    _, snapshot, parsed, _, comment, secret = loader.create_fixture(args.writer, case_root, env)
    package = session.package_backend()
    cipher = metadata = secret_read = master = process = channel = peer = connection = None
    pidfds = {}
    observed = False
    try:
        original = make_record(snapshot, parsed, comment, "complete")
        record = make_record(snapshot, parsed, comment, case)
        check_record_change(record, original, case)
        cipher, _ = package.make_sealed_key_memfd(snapshot["key_bytes"])
        require(loader.fcntl.fcntl(cipher, loader.fcntl.F_GET_SEALS) & loader.SEALS == loader.SEALS,
                "ciphertext seals differ")
        metadata = loader.sealed_metadata(record)
        secret_read, secret_write = os.pipe2(os.O_CLOEXEC)
        try:
            require(os.write(secret_write, secret) == len(secret), "secret pipe write differs")
        finally:
            os.close(secret_write)
        loader.wipe(secret)
        channel, peer = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        channel.settimeout(25)
        agent_fd, loader_fd = provider.files["ssh-agent"].fd, provider.files["ssh-add"].fd
        socket_path = case_root / "agent.sock"
        arguments = [str(args.launcher_helper), "--creation-binding-case", case]
        descriptors = (agent_fd, loader_fd, cipher, secret_read, metadata, peer.fileno())
        for name, descriptor in zip(("agent", "loader", "cipher", "secret", "fixture", "observer"), descriptors):
            arguments.extend(("--" + name + "-fd", str(descriptor)))
        arguments.extend(("--lease-dir-fd", str(snapshot["directory_fd"]), "--socket", str(socket_path)))
        provider.revalidate()
        state["uncertain"] = True
        process = subprocess.Popen(arguments, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, close_fds=True, pass_fds=descriptors + (snapshot["directory_fd"],))
        peer.close()
        peer = None
        os.close(secret_read)
        secret_read = None
        PHASE = "startup"
        master = loader.receive_master(channel)
        identity = parse_startup(channel.recv(8193), "prompt")
        require(identity["creator_tid"] == process.pid, "native creator differs from retained helper")
        for name in ("agent", "loader"):
            pid = identity[name + "_pid"]
            pidfds[name] = os.pidfd_open(pid)
            life.inspect_agent(pid, life.process_start(pid), str(provider.files[
                "ssh-agent" if name == "agent" else "ssh-add"].path), process.pid)
        slave = Path("/proc", str(identity["loader_pid"]), "fd/0").stat()
        require(stat.S_ISCHR(slave.st_mode) and slave.st_uid == os.geteuid()
                and stat.S_IMODE(slave.st_mode) == 0o600, "native private PTY differs")
        package.revalidate_retained_package(snapshot)
        channel.sendall(b"H")
        parse_startup(channel.recv(8193), "handoff", identity)
        status = Path("/proc", str(identity["loader_pid"]), "status").read_text(encoding="ascii")
        require(any(line.startswith("State:") and line.split()[1] == "T" for line in status.splitlines()),
                "native loader stop was not observed")
        channel.sendall(b"L")
        PHASE = "binding"
        result = parse_result(channel.recv(8193), case, identity, record)
        require(native.identities(socket_path) == [(parsed["public_blob"], comment.encode("ascii"))],
                "loaded original identity differs")
        check_record_change(record, original, case)
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(3)
        connection.connect(str(socket_path))
        if case == "complete":
            openssl_verify(verifier, case_root, env, parsed["public_key"], bytes.fromhex(result["challenge"]),
                           bytes.fromhex(result["signature"]), "creation")
        package.revalidate_retained_package(snapshot)
        channel.sendall(b"V")
        PHASE = "closure"
        final = parse_result(channel.recv(8193), case, identity, record, final=True)
        require(all(final[key] == result[key] for key in RESULT_FIELDS - {"stage"}),
                "creation result changed during closure")
        output = spawn.bounded_output(process, time.monotonic() + 5)
        require(process.wait(timeout=3) == 0 and parse_result(output, case, identity, record, final=True) == final,
                "creation helper stdout or exit differs")
        require(all(life.pidfd_ready(fd, 0) for fd in pidfds.values()), "native exits remain unobserved")
        require(os.isatty(master) and spawn.connection_refused(connection, socket_path),
                "post-close native capability remains usable")
        package.revalidate_retained_package(snapshot)
        provider.revalidate()
        verifier.revalidate()
        observed = True
        state["uncertain"] = False
        return dict(final, original_identity_observed=True, independent_signature_verified=case == "complete",
                    independent_exits_observed=True,
                    altered_message_refused=case == "complete", retained_connection_refused=True,
                    new_connection_refused=True, pty_master_retained_through_exit=True,
                    pty_slave_private=True, sealed_snapshot=True, lease_revalidated=True)
    finally:
        loader.wipe(secret)
        if not observed:
            state["uncertain"] = True
            UNCERTAIN.append((snapshot, cipher, metadata, secret_read, master, process, channel, peer, connection, pidfds))
        else:
            for descriptor in (*pidfds.values(), cipher, metadata, secret_read, master):
                if descriptor is not None:
                    os.close(descriptor)
            package.close_retained_package(snapshot)
            for value in (channel, peer, connection, process.stdout, process.stderr):
                if value is not None:
                    value.close()


def require_complete_cases(cases):
    require(type(cases) is list and len(cases) == len(CASES)
            and {value.get("case") for value in cases} == set(CASES)
            and all(value.get("status") == "observed" for value in cases), "creation case coverage differs")


def main():
    global PHASE
    parser = argparse.ArgumentParser()
    for name in ("writer", "launcher-helper", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--verifier", required=True)
    args = parser.parse_args()
    report = {"version": 1, "experiment": "rust-creation-binding", "status": "incomplete", "cases": [],
              **dict.fromkeys(FLAGS, False), "sentinel_survived": False}
    state = {"uncertain": False}
    provider = verifier = sentinel = snapshot = root = secret = None
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1", "disposable Linux is required")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer no_new_privs differs")
        report["resource_limits"] = life.require_resources(expected_script="rust_creation_binding_probe.py")
        life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
        session.require_report_directory(args.report)
        session.require_store(args.store_root)
        require(args.verifier == "/usr/bin/openssl", "only the native OpenSSL verifier is accepted")
        verifier = providers.RetainedFile(args.verifier, executable=True)
        report["openssl_sha256"] = verifier.sha256
        root = Path(tempfile.mkdtemp(prefix="rust-creation-", dir=args.store_root))
        root.chmod(0o700)
        package = session.package_backend()
        tree_identity = package.validate_safe_tree_root(root)
        (root / "home").mkdir(mode=0o700)
        env = life.private_env(root)
        provider = providers.Provider()
        provider.open()
        version, architecture = provider.package(env)
        report["provenance"] = {"package_version": version, "package_arch": architecture, "components": provider.components()}
        sources = {"helper": args.launcher_helper, "writer": args.writer, "observer": Path(__file__),
                   "provider_source": Path(providers.__file__), "loader_source": Path(loader.__file__)}
        hashes = {key + "_sha256": spawn.file_hash(path) for key, path in sources.items()}
        report.update(hashes)
        sentinel_root = root / "sentinel"
        sentinel_root.mkdir(mode=0o700)
        (sentinel_root / "home").mkdir(mode=0o700)
        path, snapshot, parsed, challenge, _, secret = loader.create_fixture(args.writer, sentinel_root, env)
        sentinel = providers.OwnedAgent(provider, root, "sentinel")
        sentinel.start()
        code, _ = provider.execute("ssh-add", ["-k", str(path / "key.openssh")], life.private_env(root, sentinel.path), responses=[secret])
        require(code == 0, "independent sentinel load failed")
        loader.wipe(secret)
        secret = None
        for case in CASES:
            pending = {"case": case, "status": "incomplete"}
            report["cases"].append(pending)
            pending.update(run_case(args, provider, verifier, root, case, state))
            PHASE = "sentinel"
            require(sentinel.process.poll() is None and not life.pidfd_ready(sentinel.pidfd, 0), "independent sentinel was affected")
            signature = native.sign(sentinel.path, parsed["public_blob"], native.canonical(challenge))
            openssl_verify(verifier, root, env, parsed["public_key"], native.canonical(challenge), signature, "sentinel-" + case)
            package.revalidate_retained_package(snapshot)
        require_complete_cases(report["cases"])
        require(all(spawn.file_hash(path) == hashes[key + "_sha256"] for key, path in sources.items()),
                "creation probe source or artifact changed")
        verifier.revalidate()
        sentinel.stop()
        sentinel.close()
        sentinel = None
        package.close_retained_package(snapshot)
        snapshot = None
        provider.close()
        provider = None
        verifier.close()
        verifier = None
        package.cleanup_owned_tree(root, tree_identity)
        root = None
        report.update(status="observed", sentinel_survived=True, resources_closed_after_exit=True)
        life.private_write(args.report, json.dumps(report, sort_keys=True, indent=2).encode("ascii") + b"\n")
        return 0
    except Exception as error:
        loader.wipe(secret)
        report.update(status="incomplete", failure={"phase": PHASE, "kind": type(error).__name__})
        UNCERTAIN.append((provider, verifier, sentinel, snapshot, root))
        try:
            life.private_write(args.report, json.dumps(report, sort_keys=True).encode("ascii") + b"\n")
        except OSError:
            pass
        print("Rust creation-binding experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
