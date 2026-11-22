#!/usr/bin/env python3
"""Disposable generated-secret delivery checks; reports contain metadata only."""

import argparse
import base64
import copy
import errno
import hashlib
import json
import math
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time

import heap_probe as heap
import lifecycle_probe as life
import openssh_probe as native
import provider_probe as provider_tools
import session_probe as session


ACK = b"CREATE IN TRUSTED TERMINAL"
ACK_PROMPT = b"Type CREATE IN TRUSTED TERMINAL to continue: "
SECRET_MARKER = b"Generated device key unlock secret: "
REENTRY_MARKER = b"\nRe-enter generated secret: "
OUTPUT_LIMIT = 32768
CASE_DEADLINE = 25
CASES = ("valid-generated-package", "wrong-ack", "eof-before-ack", "term-before-ack",
         "mismatch", "eof-confirmation", "term-confirmation", "terminal-hangup",
         "no-tty", "unset-disposable")
NO_DISCLOSURE = {"wrong-ack", "eof-before-ack", "term-before-ack", "no-tty", "unset-disposable",
                 "deadline-acknowledgement", "terminal-drift-acknowledgement"}
INTERRUPTIONS = {"interrupted-" + point: point for point in
                 ("key-synced", "manifest-synced", "before-publish", "after-publish")}
RETURNED_ERRORS = {"returned-" + point: point for point in
                   ("key-synced", "manifest-synced", "before-publish", "after-publish")}
PUBLICATION_ERROR_CASES = (CASES[0], *RETURNED_ERRORS)
REFUSAL_MESSAGE = b"Experimental OpenSSH writer refused or failed.\n"
INDETERMINATE_MESSAGE = (b"Experimental OpenSSH package committed; durability is indeterminate. "
                         b"Do not overwrite or retry automatically.\n")
DRIFT_CASES = {"terminal-drift-acknowledgement", "terminal-drift-confirmation"}
FAILURE_CASES = (CASES[0], "terminal-drift-acknowledgement", "terminal-drift-confirmation", *INTERRUPTIONS)
DEADLINE_CASES = {"acknowledgement": "deadline-acknowledgement", "confirmation": "deadline-confirmation"}
DEADLINE_BOUNDS_MS = {"deadline-acknowledgement": (59000, 80000),
                      "deadline-confirmation": (179000, 200000)}
CURRENT_REPORT = None
CURRENT_PHASE = "preflight"
UNCERTAIN_BUNDLES = []


class ProbeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def wipe(value):
    if isinstance(value, bytearray):
        value[:] = b"\0" * len(value)


def safe_error(error, phase):
    # Never serialize exception text: an unexpected dependency may include input.
    return {"phase": phase, "kind": type(error).__name__}


def file_sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_disclosure(output):
    require(output.count(SECRET_MARKER) <= 1, "duplicate disclosure was refused")
    start = output.find(SECRET_MARKER)
    if start < 0:
        return None
    start += len(SECRET_MARKER)
    end = output.find(REENTRY_MARKER, start)
    if end < 0:
        return None
    value = bytearray(output[start:end])
    # A PTY's standard output processing may turn LF into CRLF.
    if value.endswith(b"\r"):
        del value[-1:]
    if len(value) != 64 or any(item not in b"0123456789abcdef" for item in value):
        wipe(value)
        raise ProbeError("disclosure encoding was refused")
    return value


def case_observation(name, code, created, restored, disclosures, echo, foreground, private_arguments,
                     standard_streams_clean=True):
    return {"case": name, "status": "observed", "exit_code": code,
            "package_created": created, "terminal_restored": restored, "disclosures": disclosures,
            "echo_disabled": echo, "foreground": foreground,
            "private_arguments": private_arguments, "process_metadata_checked": private_arguments is True,
            "standard_streams_clean": standard_streams_clean}


def selected_cases(deadline_case, failure_matrix=False, publication_errors=False):
    require(type(failure_matrix) is bool and type(publication_errors) is bool
            and sum((deadline_case is not None, failure_matrix, publication_errors)) <= 1,
            "generated-secret matrices must be selected separately")
    require(deadline_case is None or deadline_case in DEADLINE_CASES, "deadline selection was refused")
    if failure_matrix:
        return FAILURE_CASES
    if publication_errors:
        return PUBLICATION_ERROR_CASES
    return CASES if deadline_case is None else (CASES[0], DEADLINE_CASES[deadline_case])


def validate_case(value):
    name = value.get("case")
    require(name in (*CASES, *DEADLINE_BOUNDS_MS, *FAILURE_CASES, *RETURNED_ERRORS)
            and value.get("status") == "observed", "case identity was refused")
    require(type(value.get("exit_code")) is int, "case exit code was refused")
    for field in ("package_created", "process_metadata_checked", "standard_streams_clean"):
        require(type(value.get(field)) is bool, "case boolean metadata was refused")
    headless = name in ("no-tty", "unset-disposable")
    for field in ("echo_disabled", "foreground"):
        require(value.get(field) is (None if headless else True), "terminal boundary was not established")
    require(value.get("private_arguments") is (None if name in NO_DISCLOSURE else True)
            and value["process_metadata_checked"] is (name not in NO_DISCLOSURE)
            and value["standard_streams_clean"], "secret delivery boundary was not established")
    expected_disclosures = 0 if name in NO_DISCLOSURE else 1
    require(type(value.get("disclosures")) is int and value["disclosures"] == expected_disclosures,
            "case disclosure count differs")
    if name in ("terminal-hangup", "no-tty", "unset-disposable"):
        require(value.get("terminal_restored") is None, "unavailable terminal cannot claim restoration")
    else:
        require(value.get("terminal_restored") is True, "terminal restoration was not established")
    positive = name == "valid-generated-package"
    committed = name in ("interrupted-after-publish", "returned-after-publish")
    require((value["exit_code"] == 0) is positive and value["package_created"] is (positive or committed),
            "case publication or refusal differs")
    if name in DRIFT_CASES:
        require(value.get("attrs_changed") is True and value["exit_code"] == 1,
                "terminal attribute drift was not observed as a reader refusal")
    if name in INTERRUPTIONS:
        require(value["exit_code"] == 77 and value.get("fault_point") == INTERRUPTIONS[name],
                "publication interruption was not observed")
        require(type(value.get("preparations_remaining")) is int
                and value["preparations_remaining"] == (0 if committed else 1)
                and value.get("package_check_accepted") is (True if committed else None)
                and value.get("filesystem_secret_absent") is True,
                "interrupted publication state differs")
        if committed:
            require(value.get("native_public_extraction_matches") is True,
                    "committed interrupted package was not decrypted")
    if name in RETURNED_ERRORS:
        require(value["exit_code"] == (2 if committed else 1)
                and value.get("error_point") == RETURNED_ERRORS[name],
                "publication error return was not observed")
        require(type(value.get("preparations_remaining")) is int and value["preparations_remaining"] == 0
                and value.get("package_check_accepted") is (True if committed else None)
                and value.get("filesystem_secret_absent") is True
                and value.get("indeterminate_durability_reported") is committed,
                "returned publication state differs")
        if committed:
            require(value.get("native_public_extraction_matches") is True,
                    "committed returned-error package was not decrypted")
    if name in DEADLINE_BOUNDS_MS:
        elapsed = value.get("delay_elapsed_ms")
        low, high = DEADLINE_BOUNDS_MS[name]
        require(type(elapsed) in (int, float) and math.isfinite(elapsed) and low <= elapsed <= high,
                "real deadline observation is outside its declared bounds")
    return True


def publication_error_reported(name, standard, stdout):
    require(name in RETURNED_ERRORS and not stdout, "publication error output channel differs")
    committed = name == "returned-after-publish"
    expected = INDETERMINATE_MESSAGE if committed else REFUSAL_MESSAGE
    require(standard == expected, "publication error diagnostic differs")
    return committed


def proc_secret_absent(pid, secret):
    for name in ("cmdline", "environ"):
        path = Path("/proc") / str(pid) / name
        with path.open("rb") as source:
            data = source.read(65537)
        require(len(data) <= 65536 and secret not in data, "child process metadata boundary failed")
    return True


def reap(process):
    if process.poll() is None:
        process.kill()
    return process.wait(timeout=2)


def child_setup(slave):
    import fcntl
    import termios
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)


def send(descriptor, payload, deadline):
    position = 0
    while position < len(payload):
        require(time.monotonic() < deadline, "PTY input deadline expired")
        try:
            count = os.write(descriptor, payload[position:])
        except BlockingIOError:
            with selectors.DefaultSelector() as selector:
                selector.register(descriptor, selectors.EVENT_WRITE)
                selector.select(0.02)
            continue
        require(count > 0, "PTY input made no progress")
        position += count


def drift_terminal_input(slave, terminal):
    before = terminal.tcgetattr(slave)
    changed = copy.deepcopy(before)
    changed[0] ^= terminal.ICRNL
    terminal.tcsetattr(slave, terminal.TCSANOW, changed)
    actual = terminal.tcgetattr(slave)
    require(actual != before and actual[0] == changed[0] and actual[3] == before[3]
            and not actual[3] & (terminal.ECHO | getattr(terminal, "ECHONL", 0)),
            "terminal input drift could not be established safely")
    return True


def inspect_interrupted_files(parent, identity, secret):
    package = session.package_backend()
    require(package.validate_safe_tree_root(parent) == identity, "interruption parent changed")
    pending = [parent]
    members = total = preparations = 0
    while pending:
        directory = pending.pop()
        for entry in directory.iterdir():
            members += 1
            require(members <= 8, "interrupted fixture has too many members")
            before = entry.lstat()
            require(before.st_uid == os.geteuid() and not before.st_mode & 0o077,
                    "interrupted fixture metadata was refused")
            if stat.S_ISDIR(before.st_mode):
                require(directory == parent, "interrupted fixture nesting was refused")
                preparations += int(entry.name.startswith(".vivac-openssh-preparing-"))
                pending.append(entry)
            else:
                require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1,
                        "interrupted fixture member was refused")
                total += before.st_size
                require(total <= 1024 * 1024, "interrupted fixture byte limit exceeded")
                contents = bytearray(native.read_private_file(entry))
                try:
                    require(len(contents) == before.st_size and secret not in contents,
                            "interrupted fixture contains plaintext or changed size")
                    require(heap.fixture_identity(entry.lstat()) == heap.fixture_identity(before),
                            "interrupted fixture changed while reading")
                finally:
                    wipe(contents)
    require(package.validate_safe_tree_root(parent) == identity, "interruption parent changed after reading")
    return preparations


def child_environment(env, name):
    result = dict(env)
    result.pop("VIVAC_OPENSSH_PACKAGE_EXIT_AT", None)
    result.pop("VIVAC_OPENSSH_PACKAGE_ERROR_AT", None)
    if name == "unset-disposable":
        result.pop("VIVAC_OPENSSH_DISPOSABLE", None)
    if name in INTERRUPTIONS:
        result["VIVAC_OPENSSH_PACKAGE_EXIT_AT"] = INTERRUPTIONS[name]
    if name in RETURNED_ERRORS:
        result["VIVAC_OPENSSH_PACKAGE_ERROR_AT"] = RETURNED_ERRORS[name]
    return result


def terminal_case(writer, destination, env, comment, name, bundle, positive_check=None, observation=None):
    import pty
    import termios

    process = None
    writer_waited = False
    master = slave = None
    secret = None
    transcript = bytearray()
    standard = bytearray()
    standard_stdout = bytearray()
    original = None
    ack_sent = confirm_sent = False
    delay_started = None
    attrs_changed = False
    echo = foreground = True
    private_arguments = None
    hangup = name == "terminal-hangup"
    headless = name in ("no-tty", "unset-disposable")
    if headless:
        echo = foreground = None
    args = [str(writer), "--generated-package-create", str(destination), comment]
    child_env = child_environment(env, name)
    case_budget = DEADLINE_BOUNDS_MS[name][1] / 1000 if name in DEADLINE_BOUNDS_MS else CASE_DEADLINE
    deadline = time.monotonic() + case_budget
    try:
        options = {"stdin": subprocess.DEVNULL, "start_new_session": True}
        if not headless:
            master, slave = pty.openpty()
            original = copy.deepcopy(termios.tcgetattr(slave))
            options = {"stdin": slave, "preexec_fn": lambda: child_setup(slave)}
        bundle["process_exit_uncertain"] = True
        process = subprocess.Popen(args, env=child_env, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, **options)
        with selectors.DefaultSelector() as selector:
            for stream in (process.stdout, process.stderr):
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, "stdout" if stream is process.stdout else "stderr")
            if master is not None:
                os.set_blocking(master, False)
                selector.register(master, selectors.EVENT_READ, "terminal")
            while True:
                require(time.monotonic() < deadline, "generated delivery case exceeded its deadline")
                events = selector.select(0.02)
                for key, _ in events:
                    try:
                        chunk = os.read(key.fd, 4096)
                    except OSError as error:
                        if error.errno == errno.EIO and key.data == "terminal":
                            selector.unregister(key.fileobj)
                            continue
                        raise
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    target = transcript if key.data == "terminal" else standard
                    target.extend(chunk)
                    if key.data == "stdout":
                        standard_stdout.extend(chunk)
                    require(len(transcript) + len(standard) <= OUTPUT_LIMIT, "case output exceeded its bound")
                if not headless and not ack_sent and ACK_PROMPT in transcript:
                    state = termios.tcgetattr(slave)
                    echo &= not bool(state[3] & (termios.ECHO | getattr(termios, "ECHONL", 0)))
                    foreground &= os.tcgetpgrp(master) == os.getpgid(process.pid)
                    require(echo and foreground, "acknowledgement terminal was unsafe")
                    if name == "deadline-acknowledgement":
                        delay_started = time.monotonic()
                    elif name == "terminal-drift-acknowledgement":
                        attrs_changed = drift_terminal_input(slave, termios)
                        send(master, ACK + b"\n", deadline)
                    elif name == "wrong-ack":
                        send(master, b"DECLINE THIS CREATION NOW\n", deadline)
                    elif name == "eof-before-ack":
                        send(master, b"\x04", deadline)
                    elif name == "term-before-ack":
                        os.kill(process.pid, signal.SIGTERM)
                    else:
                        send(master, ACK + b"\n", deadline)
                    ack_sent = True
                if not headless and secret is None:
                    secret = parse_disclosure(transcript)
                if secret is not None and not confirm_sent:
                    state = termios.tcgetattr(slave)
                    echo &= not bool(state[3] & (termios.ECHO | getattr(termios, "ECHONL", 0)))
                    foreground &= os.tcgetpgrp(master) == os.getpgid(process.pid)
                    private_arguments = proc_secret_absent(process.pid, secret)
                    require(echo and foreground, "confirmation terminal was unsafe")
                    if name == "deadline-confirmation":
                        delay_started = time.monotonic()
                    elif name == "terminal-drift-confirmation":
                        attrs_changed = drift_terminal_input(slave, termios)
                        send(master, secret + b"\n", deadline)
                    elif name == "terminal-hangup":
                        selector.unregister(master)
                        os.close(master)
                        master = None
                    elif name == "mismatch":
                        changed = bytearray(secret)
                        changed[0] = ord("0") if changed[0] != ord("0") else ord("1")
                        try:
                            send(master, changed + b"\n", deadline)
                        finally:
                            wipe(changed)
                    elif name == "eof-confirmation":
                        send(master, b"\x04", deadline)
                    elif name == "term-confirmation":
                        os.kill(process.pid, signal.SIGTERM)
                    else:
                        send(master, secret + b"\n", deadline)
                    confirm_sent = True
                # Continue draining after exit; never export any captured bytes.
                if process.poll() is not None and not events:
                    break
        code = process.wait(timeout=2)
        writer_waited = True
        bundle["process_exit_uncertain"] = False
        restored = None if headless or hangup else termios.tcgetattr(slave) == original
        clean = SECRET_MARKER not in standard and not re.search(rb"[0-9a-f]{64}", standard)
        if secret is not None:
            clean &= secret not in standard
            # The single intended disclosure is allowed; echoed confirmation is not.
            require(transcript.count(secret) == 1, "generated secret was repeated on the terminal")
        result = case_observation(name, code, destination.exists(), restored,
                                  transcript.count(SECRET_MARKER), echo, foreground,
                                  private_arguments, bool(clean))
        if name in DRIFT_CASES:
            result["attrs_changed"] = attrs_changed
        if name in DEADLINE_BOUNDS_MS:
            result["delay_elapsed_ms"] = (None if delay_started is None else
                                          round((time.monotonic() - delay_started) * 1000, 3))
        if observation is not None:
            observation.update(result)
        if name in INTERRUPTIONS:
            result.update(fault_point=INTERRUPTIONS[name], preparations_remaining=None,
                          package_check_accepted=None, filesystem_secret_absent=False)
            if observation is not None:
                observation.update(result)
            require(secret is not None, "interrupted writer had no observed secret")
            package = session.package_backend()
            identity = bundle["interruption_parents"][name]
            result["preparations_remaining"] = inspect_interrupted_files(destination.parent, identity, secret)
            result["filesystem_secret_absent"] = True
            if observation is not None:
                observation.update(result)
            if name == "interrupted-after-publish":
                checked, _ = heap.quiescent_call(bundle, lambda: package.check(writer, destination, env))
                result["package_check_accepted"] = checked == 0
                if observation is not None:
                    observation.update(result)
        if name in RETURNED_ERRORS:
            result.update(error_point=RETURNED_ERRORS[name], preparations_remaining=None,
                          package_check_accepted=None, filesystem_secret_absent=False,
                          indeterminate_durability_reported=False)
            if observation is not None:
                observation.update(result)
            result["indeterminate_durability_reported"] = publication_error_reported(name, standard, standard_stdout)
            require(secret is not None, "returned-error writer had no observed secret")
            package = session.package_backend()
            identity = bundle["interruption_parents"][name]
            result["preparations_remaining"] = inspect_interrupted_files(destination.parent, identity, secret)
            result["filesystem_secret_absent"] = True
            if observation is not None:
                observation.update(result)
            if name == "returned-after-publish":
                checked, _ = heap.quiescent_call(bundle, lambda: package.check(writer, destination, env))
                result["package_check_accepted"] = checked == 0
                if observation is not None:
                    observation.update(result)
        native_control = ((name == "valid-generated-package" and code == 0)
                          or name in ("interrupted-after-publish", "returned-after-publish"))
        if native_control and positive_check is not None:
            require(secret is not None, "successful writer had no observed secret")
            require(time.monotonic() < deadline, "positive control exceeded case deadline")
            result["native_public_extraction_matches"] = positive_check(destination, secret)
            if observation is not None:
                observation.update(result)
            require(time.monotonic() < deadline, "positive control exceeded case deadline")
        return result
    except BaseException:
        if process is not None and not writer_waited:
            try:
                reap(process)
                bundle["process_exit_uncertain"] = False
            except (OSError, subprocess.SubprocessError):
                pass
        raise
    finally:
        if process is not None:
            for stream in (process.stdout, process.stderr):
                if stream is not None:
                    stream.close()
        for descriptor in (master, slave):
            if descriptor is not None:
                os.close(descriptor)
        wipe(secret)
        wipe(transcript)
        wipe(standard)
        wipe(standard_stdout)


def native_public_control(provider, bundle, destination, secret, env):
    parsed = native.parse_key(destination / "key.openssh")
    expected = b"ssh-ed25519 " + base64.b64encode(parsed["public_blob"])
    code, output = heap.quiescent_call(bundle, lambda: provider.execute(
        "ssh-keygen", ["-y", "-f", str(destination / "key.openssh")], env, responses=[secret]))
    require(code == 0 and secret not in output and output.count(expected) == 1,
            "native public extraction did not match the generated package")
    return True


def finish(bundle, primary):
    if primary is not None:
        CURRENT_REPORT["primary_failure"] = safe_error(primary, CURRENT_PHASE)
    if bundle["process_exit_uncertain"]:
        UNCERTAIN_BUNDLES.append(bundle)
        error = ProbeError("native child exit remains uncertain")
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
        CURRENT_REPORT["cleanup_failure"] = safe_error(error, "cleanup-resources")
        if primary is None:
            raise


def run_probe(writer, verifier, store, deadline_case=None, failure_matrix=False, publication_errors=False):
    global CURRENT_REPORT, CURRENT_PHASE
    report = {"version": 1, "experiment": "generated", "status": "incomplete", "complete": False,
              "custody_accredited": False, "performance_admitted": False, "kdf_policy_adopted": False,
              "native_unlock_tested": False, "production_launcher_tested": False,
              "native_public_extraction_tested": False, "agent_load_tested": False,
              "native_public_extraction_matches": False,
              "deadline_expiry_tested": False, "control_kind": "valid-package-check",
              "retained_control_accepted": False, "cases": []}
    cases = selected_cases(deadline_case, failure_matrix, publication_errors)
    if deadline_case is not None:
        report.update(experiment="generated-deadline", deadline_case=deadline_case)
    if failure_matrix:
        report.update(experiment="generated-failures", failure_seams_tested=False,
                      terminal_replacement_tested=False, fsync_failure_tested=False)
    if publication_errors:
        report.update(experiment="generated-publication-errors", publication_error_returns_tested=False,
                      terminal_replacement_tested=False, fsync_failure_tested=False)
    CURRENT_REPORT = report
    life.require_isolation()
    life.require_unprivileged()
    require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer is missing no_new_privs")
    report["resource_limits"] = life.require_resources(expected_script="generated_probe.py")
    session.require_store(store)
    package = session.package_backend()
    root = Path(tempfile.mkdtemp(prefix="vivac-generated-", dir="/tmp"))
    root.chmod(0o700)
    store_root = store / "generated-fixtures"
    store_root.mkdir(mode=0o700)
    roots = [(root, package.validate_safe_tree_root(root)),
             (store_root, package.validate_safe_tree_root(store_root))]
    provider = provider_tools.Provider()
    bundle = {"provider": provider, "roots": roots, "process_exit_uncertain": False,
              "interruption_parents": {}}
    primary = None
    try:
        CURRENT_PHASE = "provider-admission"
        (root / "home").mkdir(mode=0o700)
        env = life.private_env(root)
        provider.open()
        version, architecture = heap.quiescent_call(bundle, lambda: provider.package(env))
        report["provenance"] = {"os_id": "ubuntu", "os_version": "24.04", "package_name": "openssh-client",
                                "package_version": version, "package_arch": architecture,
                                "components": provider.components()}
        report["helper_sha256"] = {"writer": file_sha256(writer), "verifier": file_sha256(verifier)}
        report["source_sha256"] = {"generated_probe": file_sha256(Path(__file__).resolve()),
                                   "provider_probe": file_sha256(Path(provider_tools.__file__).resolve())}
        comment = "vivac.device-key.v1:00000000000000000000000001:00000000000000000000000002"
        for name in cases:
            CURRENT_PHASE = name
            path = store_root / name
            if name in INTERRUPTIONS or name in RETURNED_ERRORS:
                path.mkdir(mode=0o700)
                bundle["interruption_parents"][name] = package.validate_safe_tree_root(path)
                path = path / "credential"
            recorded = {"case": name, "status": "incomplete"}
            report["cases"].append(recorded)
            result = terminal_case(writer, path, env, comment, name, bundle,
                                   lambda target, secret: native_public_control(provider, bundle, target, secret, env),
                                   observation=recorded)
            validate_case(result)
            if name == "valid-generated-package":
                code, _ = heap.quiescent_call(bundle, lambda: package.check(writer, path, env))
                result["structural_check_accepted"] = code == 0
                recorded.update(result)
                require(code == 0, "generated package failed structural checking")
                report["native_public_extraction_matches"] = result.get("native_public_extraction_matches") is True
                report["native_public_extraction_tested"] = report["native_public_extraction_matches"]
        CURRENT_PHASE = "retained-control"
        code, _ = heap.quiescent_call(bundle, lambda: package.check(writer, store_root / CASES[0], env))
        report["retained_control_accepted"] = code == 0
        require(code == 0, "retained generated package was refused")
        provider.revalidate()
        report["provenance"]["components"] = provider.components()
        require(len(report["cases"]) == len(cases) and all(validate_case(case) for case in report["cases"]),
                "generated-secret case matrix is incomplete")
    except BaseException as error:
        primary = error
        raise
    finally:
        finish(bundle, primary)
    report.update(status="observed", complete=True, resources_closed_after_measurement=True,
                  deadline_expiry_tested=deadline_case is not None)
    if failure_matrix:
        report["failure_seams_tested"] = True
    if publication_errors:
        report["publication_error_returns_tested"] = True
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("writer", "verifier", "store-root", "report"):
        parser.add_argument("--" + name, type=Path, required=True)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--deadline-case", choices=sorted(DEADLINE_CASES))
    modes.add_argument("--failure-matrix", action="store_true")
    modes.add_argument("--publication-errors", action="store_true")
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "this experiment requires explicitly disposable Linux")
        session.require_report_directory(args.report)
        result = run_probe(args.writer.resolve(strict=True), args.verifier.resolve(strict=True), args.store_root,
                           args.deadline_case, args.failure_matrix, args.publication_errors)
        life.private_write(args.report, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("Generated-secret delivery observations recorded; custody_accredited=false")
        return 0
    except Exception as error:
        try:
            session.require_report_directory(args.report)
            result = dict(CURRENT_REPORT or {})
            experiment = ("generated-publication-errors" if args.publication_errors else
                          "generated-failures" if args.failure_matrix else
                          "generated-deadline" if args.deadline_case else "generated")
            result.update(version=1, experiment=experiment,
                          status="incomplete", complete=False,
                          custody_accredited=False, phase=CURRENT_PHASE, failure=safe_error(error, CURRENT_PHASE))
            life.private_write(args.report, json.dumps(result, sort_keys=True).encode("ascii") + b"\n")
        except (OSError, session.ProbeError):
            pass
        print("Generated-secret delivery experiment failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
