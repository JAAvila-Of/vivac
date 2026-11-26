#!/usr/bin/env python3
"""Disposable concurrent signing/lock admission experiment."""

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import queue
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

import lifecycle_probe as life
import openssh_probe as native
import session_probe as session_fixture


TIMEOUT = 15
CASES = ("completed-before-lock", "lock-before-native-send", "lock-native-response-pending",
         "lock-after-native-reply", "lock-before-publication", "parallel-lock-callers",
         "lease-change-before-publication")
CURRENT_STAGE = "preflight"
CURRENT_REPORT = None
THREAD_PARENT_READY = False
WORK_UNCERTAIN = False


class ProbeError(Exception):
    pass


class AdmissionClosed(ProbeError):
    pass


class Busy(ProbeError):
    pass


class Cancelled(ProbeError):
    pass


class VerifierUncertain(ProbeError):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def remaining(deadline):
    value = deadline - time.monotonic()
    require(value > 0, "lock exceeded its total deadline")
    return value


@contextmanager
def state_until(condition, deadline):
    require(condition.acquire(timeout=remaining(deadline)), "lock state acquisition exceeded its deadline")
    try:
        remaining(deadline)
        yield
    finally:
        condition.release()


def prepare_thread_parent():
    global THREAD_PARENT_READY
    import resource

    require(threading.current_thread() is threading.main_thread() and threading.active_count() == 1,
            "native setup requires the sole main thread")
    session_fixture.package_backend()
    native.child_setup()
    require(resource.getrlimit(resource.RLIMIT_CORE) == (0, 0), "parent core limit was not disabled")
    require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "parent is missing no_new_privs")
    THREAD_PARENT_READY = True


def threaded_verify(helper, root, env, document, name, expected_success):
    require(THREAD_PARENT_READY, "thread-safe verifier parent was not prepared")
    payload = json.dumps(document, separators=(",", ":")).encode("utf-8")
    require(len(payload) <= 4096, "proof exceeded its bound")
    path = root / (name + ".json")
    native.private_write(path, payload)
    process = None
    try:
        # Limits are inherited from the prepared parent; no Python runs before exec.
        process = subprocess.Popen([str(helper), str(path)], env=env, close_fds=True,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL)
        code = process.wait(timeout=TIMEOUT)
        require((code == 0) == expected_success, "proof verifier returned an unexpected result")
    finally:
        if process is not None and process.poll() is None:
            try:
                try:
                    process.kill()
                except ProcessLookupError:
                    pass
                process.wait(timeout=life.DEATH_TIMEOUT)
            except (OSError, subprocess.TimeoutExpired) as error:
                raise VerifierUncertain("verifier exit could not be observed") from error


class Gate:
    def __init__(self, resource, *, timeout=TIMEOUT):
        require(resource.state == "READY", "gate requires an admitted session")
        self.resource = resource
        self.timeout = timeout
        self.condition = threading.Condition()
        self.shutdown = threading.Lock()
        self.responses = queue.Queue(maxsize=1)
        self.closing = threading.Event()
        self.drain_wait_started = threading.Event()
        self.state = "READY"
        self.epoch = 0
        self.active_thread = None
        self.sequence = 0
        self.close_acknowledged = False
        self.stats = {"admitted": 0, "busy_refused": 0, "closed_refused": 0,
                      "native_requests_sent": 0, "native_replies_received": 0,
                      "proofs_verified": 0, "proofs_published": 0,
                      "lock_callers": 0, "lock_acknowledgments": 0,
                      "drain_waits": 0, "resources_closed_after_drain": False}

    def snapshot(self):
        with self.condition:
            return dict(self.stats, state=self.state, active=self.active_thread is not None,
                        close_acknowledged=self.close_acknowledged)

    def _increment(self, name):
        with self.condition:
            self.stats[name] += 1

    def _check(self, epoch):
        with self.condition:
            if self.state != "READY" or self.epoch != epoch:
                raise Cancelled("request cancelled by lock")

    def _native_sign(self, public_blob, message, epoch, hook):
        self._check(epoch)
        self.resource._validate()
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            connection.settimeout(TIMEOUT)
            connection.connect(str(self.resource.socket_path))
            hook("before-native-send")
            self._check(epoch)
            self.resource._validate()
            request = b"\x0d" + native.wire_string(public_blob) + native.wire_string(message) + struct.pack(">I", 0)
            connection.sendall(struct.pack(">I", len(request)) + request)
            self._increment("native_requests_sent")
            hook("native-response-pending")
            self._check(epoch)
            length = struct.unpack(">I", native.receive_exact(connection, 4, time.monotonic() + TIMEOUT))[0]
            require(0 < length <= native.FRAME_LIMIT, "native reply length was refused")
            reader = native.Reader(native.receive_exact(connection, length, time.monotonic() + TIMEOUT))
            require(reader.take(1) == b"\x0e", "native signature was refused")
            signature = native.Reader(reader.string(128))
            reader.finish()
            require(signature.string(32) == b"ssh-ed25519", "native signature algorithm was refused")
            value = signature.string(64)
            signature.finish()
            require(len(value) == 64, "native signature length was refused")
            self._increment("native_replies_received")
            hook("after-native-reply")
            self._check(epoch)
            self.resource._validate()
            return value
        finally:
            connection.close()

    def _proof(self, epoch, hook):
        resource = self.resource
        name = "concurrent-proof-" + str(self.sequence)
        path = resource.root / (name + ".json")
        uncertain = False
        try:
            accepted = session_fixture.package_backend().produce_loaded_proof(
                resource.verifier, resource.root, resource.env, resource.socket_path, resource.snapshot,
                resource.expected_public_blob, resource.expected_comment, name,
                lambda public, message: self._native_sign(public, message, epoch, hook),
                verifier_runner=threaded_verify,
            )
            require(accepted, "loaded identity proof was refused")
            self._increment("proofs_verified")
            raw = native.read_private_file(path)
            require(len(raw) <= 4096, "proof exceeded its bound")
            proof = json.loads(raw.decode("utf-8"))
            hook("before-publication")
            return proof
        except VerifierUncertain:
            uncertain = True
            raise
        finally:
            if not uncertain and path.exists():
                path.unlink()

    def sign(self, *, hook=None, work=None):
        global WORK_UNCERTAIN
        hook = (lambda stage: None) if hook is None else hook
        with self.condition:
            if self.state != "READY":
                self.stats["closed_refused"] += 1
                raise AdmissionClosed("session admission is closed")
            if self.active_thread is not None or not self.responses.empty():
                self.stats["busy_refused"] += 1
                raise Busy("one request or unpublished response is already pending")
            epoch = self.epoch
            self.active_thread = threading.get_ident()
            self.sequence += 1
            self.stats["admitted"] += 1
        failure = None
        external_close = False
        uncertain = False
        try:
            self.resource._validate()
            proof = work() if work is not None else self._proof(epoch, hook)
            self.resource._validate()
            with self.condition:
                self._check(epoch)
                # Availability in this bounded queue is the fixture's publication point.
                self.responses.put_nowait(proof)
                self.stats["proofs_published"] += 1
        except BaseException as error:
            with self.condition:
                uncertain = isinstance(error, VerifierUncertain)
                WORK_UNCERTAIN = WORK_UNCERTAIN or uncertain
                external_close = self.state != "READY" or self.epoch != epoch
                failure = Cancelled("request cancelled by lock") if external_close and not uncertain else error
                if not external_close:
                    self.state = "CLOSING"
                    self.epoch += 1
                    self.closing.set()
                    self.condition.notify_all()
        finally:
            # All socket, file and verifier access has finished before releasing ownership.
            with self.condition:
                if not uncertain:
                    self.active_thread = None
                self.condition.notify_all()
        if failure is not None:
            if not external_close and not uncertain:
                self.lock()
            raise failure

    def take_proof(self):
        return self.responses.get_nowait()

    def wait_lock_callers(self, count):
        with self.condition:
            require(self.condition.wait_for(lambda: self.stats["lock_callers"] >= count, self.timeout),
                    "parallel lock callers did not enter the gate")

    def lock(self):
        deadline = time.monotonic() + self.timeout
        with state_until(self.condition, deadline):
            require(self.active_thread != threading.get_ident(), "active signer cannot wait for itself")
            self.stats["lock_callers"] += 1
            if self.state == "READY":
                self.state = "CLOSING"
                self.epoch += 1
                self.closing.set()
            self.condition.notify_all()
        require(self.shutdown.acquire(timeout=remaining(deadline)), "another lock did not complete before its deadline")
        try:
            with state_until(self.condition, deadline):
                if self.state == "CLOSED":
                    remaining(deadline)
                    self.stats["lock_acknowledgments"] += 1
                    return True
            remaining(deadline)
            self.resource._stop_agent()
            remaining(deadline)
            with state_until(self.condition, deadline):
                if self.active_thread is not None:
                    self.stats["drain_waits"] += 1
                    self.drain_wait_started.set()
                require(self.condition.wait_for(lambda: self.active_thread is None, remaining(deadline)),
                        "active signing request did not drain")
                remaining(deadline)
                require(self.active_thread is None, "resources cannot close before request drain")
            remaining(deadline)
            require(self.resource.lock() is True and self.resource.state == "CLOSED"
                    and self.resource.close_acknowledged is True, "owned session did not acknowledge closure")
            remaining(deadline)
            with state_until(self.condition, deadline):
                remaining(deadline)
                self.state = "CLOSED"
                self.stats["resources_closed_after_drain"] = True
                self.close_acknowledged = True
                self.stats["lock_acknowledgments"] += 1
                self.condition.notify_all()
            return True
        finally:
            self.shutdown.release()


class Barrier:
    def __init__(self, stage):
        self.stage = stage
        self.reached = threading.Event()
        self.release = threading.Event()

    def __call__(self, stage):
        if stage == self.stage:
            self.reached.set()
            require(self.release.wait(TIMEOUT), "fixture barrier was not released")


class Call:
    def __init__(self, target):
        self.error = None
        self.result = None
        self.done = threading.Event()
        def invoke():
            try:
                self.result = target()
            except BaseException as error:
                self.error = error
            finally:
                self.done.set()
        self.thread = threading.Thread(target=invoke)

    def start(self):
        self.thread.start()
        return self


def join_calls(calls):
    deadline = time.monotonic() + TIMEOUT
    for call in calls:
        call.thread.join(max(0, deadline - time.monotonic()))
    require(not any(call.thread.is_alive() for call in calls), "fixture workers did not exit before deadline")


def require_error(call, expected_type):
    require(isinstance(call.error, expected_type), "fixture request reached the wrong rejection gate")


def run_case(writer, verifier, store, root, name):
    global CURRENT_STAGE
    CURRENT_STAGE = name
    require(threading.current_thread() is threading.main_thread() and threading.active_count() == 1,
            "native startup requires joined fixture workers")
    package = session_fixture.package_backend()
    case_root = Path(tempfile.mkdtemp(prefix="concurrent-case-", dir=root))
    case_root.chmod(0o700)
    root_identity = package.validate_safe_tree_root(case_root)
    (case_root / "home").mkdir(mode=0o700)
    package_path = store / name
    package.create_positive(writer, package_path, life.private_env(case_root))
    package_identity = package.validate_safe_tree_root(package_path)
    resource = session_fixture.Session(writer, verifier, package_path, case_root)
    gate = None
    connection = None
    retained_pidfd = None
    calls = []
    restore = None
    stage = {"completed-before-lock": "before-native-send", "lock-before-native-send": "before-native-send",
             "lock-native-response-pending": "native-response-pending", "lock-after-native-reply": "after-native-reply",
             "lock-before-publication": "before-publication", "parallel-lock-callers": "before-native-send",
             "lease-change-before-publication": "before-publication"}[name]
    barrier = Barrier(stage)
    try:
        receipt = resource.start()
        native.verify(verifier, case_root, resource.env, receipt, "readiness-independent", True)
        message = native.canonical(receipt["challenge"])
        connection, signature = life.open_signed_connection(resource.socket_path, resource.expected_public_blob,
                                                            resource.expected_comment, message)
        native.verify(verifier, case_root, resource.env, native.proof(receipt["challenge"], signature),
                      "retained-before-lock", True)
        retained_pidfd = os.dup(resource.pidfd)
        gate = Gate(resource)
        signer = Call(lambda: gate.sign(hook=barrier)).start()
        calls.append(signer)
        require(barrier.reached.wait(TIMEOUT), "signer did not reach the selected native seam")
        require(gate.snapshot()["active"], "signer was not active at the fixture seam")
        if name == "completed-before-lock":
            before_busy = gate.snapshot()["native_requests_sent"]
            busy = Call(gate.sign).start()
            calls.append(busy)
            join_calls([busy])
            require_error(busy, Busy)
            require(gate.snapshot()["native_requests_sent"] == before_busy,
                    "busy request reached native signing")
            barrier.release.set()
            join_calls(calls)
            require(signer.error is None, "completed request failed before lock")
            proof = gate.take_proof()
            native.verify(verifier, case_root, resource.env, proof, "published-independent", True)
            locker = Call(gate.lock).start()
            calls.append(locker)
            join_calls([locker])
            require(locker.error is None and locker.result is True, "completed request lock was not acknowledged")
        elif name == "lease-change-before-publication":
            restore = session_fixture.mutate_key(package_path)
            barrier.release.set()
            join_calls(calls)
            require_error(signer, package.LeaseInvalidated)
        else:
            count = 2 if name == "parallel-lock-callers" else 1
            lockers = [Call(gate.lock).start() for _ in range(count)]
            calls.extend(lockers)
            require(gate.closing.wait(TIMEOUT), "lock did not close admission")
            gate.wait_lock_callers(count)
            require(gate.drain_wait_started.wait(TIMEOUT), "lock did not reach its active-request drain wait")
            require(not gate.close_acknowledged and not any(call.done.is_set() for call in lockers),
                    "lock acknowledged while signer was still paused")
            require(gate.snapshot()["active"], "request drained before the selected fixture release")
            sent = gate.snapshot()["native_requests_sent"]
            late = Call(gate.sign).start()
            calls.append(late)
            join_calls([late])
            require_error(late, AdmissionClosed)
            require(gate.snapshot()["native_requests_sent"] == sent, "late request reached native signing")
            barrier.release.set()
            join_calls(calls)
            require_error(signer, Cancelled)
            require(all(call.error is None and call.result is True for call in lockers),
                    "parallel lock did not acknowledge observed closure")
        result = gate.snapshot()
        expected_sent = 0 if name in ("lock-before-native-send", "parallel-lock-callers") else 1
        expected_reply = 0 if name in ("lock-before-native-send", "parallel-lock-callers", "lock-native-response-pending") else 1
        expected_verified = 1 if name in ("completed-before-lock", "lock-before-publication", "lease-change-before-publication") else 0
        require(result["native_requests_sent"] == expected_sent
                and result["native_replies_received"] == expected_reply
                and result["proofs_verified"] == expected_verified,
                "request crossed the wrong native boundary")
        require(result["proofs_published"] == (1 if name == "completed-before-lock" else 0)
                and gate.responses.empty(), "cancelled request published a proof")
        require(result["state"] == "CLOSED" and result["close_acknowledged"] and not result["active"],
                "fixture did not reach drained terminal closure")
        require(life.pidfd_ready(retained_pidfd, 0), "lock acknowledged without observed native exit")
        require(life.request_after_death(connection, resource.expected_public_blob, message)
                in ("transport_refused", "eof", "refused"), "retained connection signed after lock")
        require(life.new_connection_after_death(resource.socket_path, resource.expected_public_blob, message)
                in ("connect_refused", "transport_refused", "eof", "refused"), "new connection signed after lock")
        if restore is not None:
            restore()
            restore = None
        rejected = Call(gate.sign).start()
        calls.append(rejected)
        join_calls([rejected])
        require_error(rejected, AdmissionClosed)
        result = gate.snapshot()
        result.update({"case": name, "selected_seam_reached": True, "workers_joined": True,
                       "selected_seam": stage,
                       "request_outcome": ("published" if signer.error is None else
                                           "lease_invalidated" if isinstance(signer.error, package.LeaseInvalidated)
                                           else "cancelled_by_lock"),
                       "closing_observed_while_paused": name not in ("completed-before-lock", "lease-change-before-publication"),
                       "request_late_refused": name not in ("completed-before-lock", "lease-change-before-publication"),
                       "drain_wait_observed": gate.drain_wait_started.is_set(),
                       "retained_connection_refused_after_lock": True, "new_connection_refused_after_lock": True,
                       "closed_request_refused": True})
        return result
    finally:
        barrier.release.set()
        join_calls(calls)
        require(not WORK_UNCERTAIN and (gate is None or not gate.snapshot()["active"]),
                "uncertain backend ownership prevents fixture cleanup")
        try:
            if gate is not None and gate.state != "CLOSED":
                gate.lock()
            elif resource.state != "CLOSED":
                resource.lock()
        finally:
            if connection is not None:
                connection.close()
            if retained_pidfd is not None:
                os.close(retained_pidfd)
            if restore is not None:
                restore()
            package.cleanup_owned_tree(package_path, package_identity)
            package.cleanup_owned_tree(case_root, root_identity)


def run_probe(writer, verifier, store):
    global CURRENT_REPORT, CURRENT_STAGE
    CURRENT_STAGE = "preflight"
    report = {"version": 1, "status": "incomplete", "custody_accredited": False,
              "concurrent_lock_tested": False, "loader_cancel_tested": False,
              "ssh_disconnect_tested": False, "cases": [], "sentinel_survived": False}
    CURRENT_REPORT = report
    life.require_isolation()
    life.require_unprivileged()
    report["resource_limits"] = life.require_resources(expected_script="concurrent_probe.py")
    session_fixture.require_store(store)
    prepare_thread_parent()
    temporary = tempfile.mkdtemp(prefix="vivac-concurrent-", dir="/tmp")
    root = Path(temporary).resolve()
    package = session_fixture.package_backend()
    root.chmod(0o700)
    root_identity = package.validate_safe_tree_root(root)
    try:
        root.chmod(0o700)
        (root / "home").mkdir(mode=0o700)
        ssh_agent, ssh_add, comment, parsed, challenge, message, env = life.create_fixture(root, writer, verifier)
        sentinel_socket = root / "sentinel.sock"
        with life.owned_sentinel(ssh_agent, root, sentinel_socket) as (sentinel, pidfd):
            life.wait_socket(sentinel_socket, sentinel, sentinel.pid)
            code, _ = native.pty_run([ssh_add, "-k", str(root / "fixture-key")],
                                     life.private_env(root, sentinel_socket), [native.PASSWORD])
            require(code == 0, "sentinel credential load failed")
            for name in CASES:
                report["cases"].append(run_case(writer, verifier, store, root, name))
                require(threading.active_count() == 1 and sentinel.poll() is None
                        and not life.pidfd_ready(pidfd, 0), "sentinel or worker lifetime changed")
                signature = native.sign(sentinel_socket, parsed["public_blob"], message)
                native.verify(verifier, root, env, native.proof(challenge, signature), "sentinel-" + name, True)
            report["sentinel_survived"] = True
    finally:
        if not WORK_UNCERTAIN and threading.active_count() == 1:
            package.cleanup_owned_tree(root, root_identity)
    report["concurrent_lock_tested"] = True
    report["workers_joined"] = True
    report["backend_exit_uncertain"] = False
    report["status"] = "observed"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--writer", type=Path, required=True)
    parser.add_argument("--verifier", type=Path, required=True)
    parser.add_argument("--store-root", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "this experiment requires an explicitly disposable Linux container")
        session_fixture.require_report_directory(args.report)
        writer, verifier = args.writer.resolve(strict=True), args.verifier.resolve(strict=True)
        require(writer.is_file() and verifier.is_file() and os.access(writer, os.X_OK)
                and os.access(verifier, os.X_OK), "required helper executable is unavailable")
        result = run_probe(writer, verifier, args.store_root)
        life.private_write(args.report, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("Concurrent observations recorded; custody_accredited=false")
        return 0
    except Exception as error:
        failure = str(error) if isinstance(error, ProbeError) else type(error).__name__
    try:
        result = dict(CURRENT_REPORT or {})
        result.update({"version": 1, "status": "incomplete", "custody_accredited": False,
                       "stage": CURRENT_STAGE, "failure": failure,
                       "backend_exit_uncertain": WORK_UNCERTAIN,
                       "workers_joined": threading.active_count() == 1})
        session_fixture.require_report_directory(args.report)
        life.private_write(args.report, json.dumps(result, sort_keys=True).encode("ascii") + b"\n")
    except (session_fixture.ProbeError, OSError):
        pass
    print("Concurrent probe failed; no result is accredited.", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
