#!/usr/bin/env python3
"""Disposable native-loader lifetime and startup cancellation experiment."""

import argparse
import array
import errno
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time

import lifecycle_probe as life
import openssh_probe as native
import session_probe as session


TIMEOUT = 15
CASES = ("load-ready-close", "cancel-at-prompt", "cancel-after-secret-handoff",
         "cancel-before-readiness", "supervisor-kill-at-prompt",
         "supervisor-kill-after-secret-handoff", "loader-death-at-prompt",
         "supervisor-kill-before-readiness")
CURRENT_STAGE = "preflight"
CURRENT_REPORT = None
CLEANUP_UNCERTAIN = False
UNCERTAIN_BUNDLES = []


class ProbeError(Exception):
    pass


class Cancelled(ProbeError):
    pass


class LoaderFailed(ProbeError):
    pass


class WorkerFailed(ProbeError):
    def __init__(self, diagnostic):
        self.diagnostic = diagnostic
        super().__init__("worker reported a bounded startup failure")


ERROR_CODES = {"native loader prompt left terminal echo enabled": "prompt-echo-enabled",
               "native loader exited before its password prompt": "loader-exited-before-prompt",
               "loader password prompt timed out": "prompt-timeout",
               "loader executor identity changed": "loader-executor-identity",
               "control channel closed": "control-channel-closed",
               "loader output exceeded its bound": "loader-output-limit",
               "native loader is unavailable": "loader-unavailable",
               "loader parent changed before native execution": "loader-parent-changed",
               "observer could not confirm all native exits": "observer-exit-uncertain"}
WORKER_PHASES = {"executor:parent-check", "executor:setsid", "executor:controlling-pty",
                 "executor:arm-parent-death", "executor:exec-native", "session-created",
                 "session-open-package", "session-launch-agent", "loader-start", "transfer-master",
                 "prompt", "secret-handoff", "before-readiness", "ready", "loader-created",
                 "loader-open-pty", "loader-spawn-executor", "loader-await-arm", "loader-await-prompt",
                 "loader-inspect-native", "loader-prompt-observed"}


def safe_error(error, phase):
    if isinstance(error, WorkerFailed):
        return dict(error.diagnostic)
    code = ERROR_CODES.get(str(error), "probe-error") if isinstance(
        error, (ProbeError, native.ProbeError, life.ProbeError, session.ProbeError)) else "os-error" if isinstance(error, OSError) else "internal-error"
    number = getattr(error, "errno", None) if isinstance(error, OSError) else None
    return {"phase": phase, "error_type": type(error).__name__, "errno": number, "code": code}


def validate_diagnostic(value):
    require(isinstance(value, dict) and set(value) == {"phase", "error_type", "errno", "code"},
            "worker diagnostic schema was refused")
    require(all(isinstance(value[key], str) and re.fullmatch(r"[A-Za-z][A-Za-z0-9_:.-]{0,79}", value[key])
                for key in ("phase", "error_type", "code")), "worker diagnostic fields were refused")
    require(value["phase"] in WORKER_PHASES and value["code"] in set(ERROR_CODES.values())
            | {"probe-error", "os-error", "internal-error"}, "worker diagnostic code was refused")
    require(value["errno"] is None or type(value["errno"]) is int and 0 <= value["errno"] <= 65535,
            "worker diagnostic errno was refused")
    return value


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def receive(connection):
    value = life.receive_json(connection, time.monotonic() + TIMEOUT)
    if isinstance(value, dict) and value.get("kind") == "worker-error":
        raise WorkerFailed(validate_diagnostic(value.get("diagnostic")))
    return value


def send_fd(connection, descriptor):
    connection.settimeout(TIMEOUT)
    require(connection.sendmsg([b"F"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS,
                                          array.array("i", [descriptor]))]) == 1,
            "PTY descriptor transfer was incomplete")


def receive_fd(connection):
    connection.settimeout(TIMEOUT)
    received = []
    accepted = False
    try:
        size = array.array("i").itemsize
        payload, controls, flags, _ = connection.recvmsg(1, socket.CMSG_SPACE(8 * size))
        valid_controls = True
        for level, kind, raw in controls:
            if level != socket.SOL_SOCKET or kind != socket.SCM_RIGHTS:
                valid_controls = False
                continue
            descriptors = array.array("i")
            descriptors.frombytes(raw[:len(raw) - len(raw) % size])
            received.extend(descriptors)
            valid_controls = valid_controls and len(raw) % size == 0
        if payload == b"E":
            require(not received and not controls and not flags & socket.MSG_CTRUNC,
                    "worker error descriptor frame was refused")
            receive(connection)
            raise ProbeError("worker error frame did not contain a diagnostic")
        require(payload == b"F" and valid_controls and not flags & socket.MSG_CTRUNC
                and len(received) == 1, "PTY descriptor message was refused")
        descriptor = received[0]
        os.set_inheritable(descriptor, False)
        require(stat.S_ISCHR(os.fstat(descriptor).st_mode) and os.isatty(descriptor)
                and Path(os.ttyname(descriptor)).name == "ptmx", "received descriptor is not a PTY master")
        accepted = True
        return descriptor
    finally:
        if not accepted:
            errors = []
            while received:
                descriptor = received.pop(0)
                try:
                    os.close(descriptor)
                except OSError as error:
                    errors.append(error)
            if errors:
                raise errors[0]


def claim_child(pid, start_time, parent):
    descriptor = os.pidfd_open(pid)
    try:
        require(life.process_start(pid) == start_time
                and life.agent_credentials(pid)["PPid"] == [parent], "native child ownership changed")
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def require_parent(expected):
    if os.getppid() != expected:
        raise life.ParentChanged("loader parent changed before native execution")


def loader_exec(args):
    import fcntl
    import termios

    control = socket.socket(fileno=args.control_fd)
    phase = "executor:parent-check"
    try:
        require_parent(args.expected_parent)
        native.child_setup()
        phase = "executor:setsid"
        os.setsid()
        phase = "executor:controlling-pty"
        fcntl.ioctl(args.slave_fd, termios.TIOCSCTTY, 0)
        for target in (0, 1, 2):
            os.dup2(args.slave_fd, target)
        if args.slave_fd > 2:
            os.close(args.slave_fd)
        require_parent(args.expected_parent)
        phase = "executor:arm-parent-death"
        require(life.configure_parent_death() == "armed", "loader parent-death link was refused")
        require_parent(args.expected_parent)
        life.send_json(control, {"kind": "loader-armed", "pid": os.getpid(), "parent": args.expected_parent})
        require(life.receive_command(control) == b"G", "loader execution barrier was not released")
        require_parent(args.expected_parent)
        phase = "executor:exec-native"
        executable = shutil.which("ssh-add", path=os.defpath)
        require(executable is not None, "native loader is unavailable")
        os.set_inheritable(args.cipher_fd, True)
        os.set_inheritable(args.control_fd, False)
        os.execve(executable, [executable, "-k", "/proc/self/fd/" + str(args.cipher_fd)],
                  life.private_env(Path(args.home), args.agent_socket))
        return 1
    except BaseException as error:
        try:
            life.send_json(control, {"kind": "worker-error", "diagnostic": safe_error(error, phase)})
        except (OSError, life.ProbeError):
            pass
        raise
    finally:
        control.close()


class OwnedLoader:
    def __init__(self, owner):
        self.owner = owner
        self.process = None
        self.pidfd = None
        self.start_time = None
        self.master = None
        self.slave = None
        self.output = bytearray()
        self.prompt_seen = False
        self.echo_disabled = False
        self.secret_writes = 0
        self.suspended = False
        self.handoff_suspended = False
        self.exit_observed = False
        self.executor_control = None
        self.phase = "loader-created"

    def start(self):
        import pty

        parent = child = None
        try:
            self.phase = "loader-open-pty"
            self.master, self.slave = pty.openpty()
            parent, child = socket.socketpair()
            arguments = [sys.executable, str(Path(__file__).resolve()), "--internal-loader-exec",
                         "--expected-parent", str(os.getpid()), "--control-fd", str(child.fileno()),
                         "--slave-fd", str(self.slave), "--cipher-fd", str(self.owner.memfd),
                         "--home", str(self.owner.root / "home"),
                         "--agent-socket", str(self.owner.socket_path)]
            self.phase = "loader-spawn-executor"
            self.process = subprocess.Popen(arguments, env=self.owner.env, stdin=subprocess.DEVNULL,
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                            close_fds=True,
                                            pass_fds=(child.fileno(), self.slave, self.owner.memfd))
            child.close()
            self.pidfd = os.pidfd_open(self.process.pid)
            self.start_time = life.process_start(self.process.pid)
            self.phase = "loader-await-arm"
            metadata = receive(parent)
            require(metadata == {"kind": "loader-armed", "pid": self.process.pid, "parent": os.getpid()},
                    "loader executor identity changed")
            parent.sendall(b"G")
            os.close(self.slave)
            self.slave = None
            os.set_blocking(self.master, False)
            self.phase = "loader-await-prompt"
            self.executor_control = parent
            self.wait_prompt()
            self.phase = "loader-inspect-native"
            life.inspect_agent(self.process.pid, self.start_time,
                               shutil.which("ssh-add", path=os.defpath), os.getpid())
            self.phase = "loader-prompt-observed"
        finally:
            self.executor_control = None
            if parent is not None:
                parent.close()
            if child is not None:
                child.close()

    def read_output(self):
        try:
            chunk = os.read(self.master, 4096)
        except BlockingIOError:
            return
        except OSError as error:
            if error.errno == errno.EIO:
                return
            raise
        self.output.extend(chunk)
        require(len(self.output) <= native.OUTPUT_LIMIT, "loader output exceeded its bound")

    def wait_prompt(self):
        import termios

        deadline = time.monotonic() + TIMEOUT
        control = self.executor_control
        while time.monotonic() < deadline:
            watched = [self.master] + ([control] if control is not None else [])
            readable = select.select(watched, [], [], 0.05)[0]
            if control is not None and control in readable:
                if control.recv(1, socket.MSG_PEEK):
                    receive(control)
                    raise ProbeError("loader executor sent an unexpected control message")
                control = None
            if self.process.poll() is not None:
                if control is not None:
                    continue
                raise LoaderFailed("native loader exited before its password prompt")
            if self.master in readable:
                self.read_output()
            if native.PROMPT.search(self.output):
                require(not termios.tcgetattr(self.master)[3] & termios.ECHO,
                        "native loader prompt left terminal echo enabled")
                self.prompt_seen = self.echo_disabled = True
                return
        raise ProbeError("loader password prompt timed out")

    def suspend(self):
        life.pidfd_signal(self.pidfd, signal.SIGSTOP)
        deadline = time.monotonic() + life.DEATH_TIMEOUT
        while time.monotonic() < deadline:
            pid, status = os.waitpid(self.process.pid, os.WNOHANG | os.WUNTRACED)
            if pid:
                if os.WIFSTOPPED(status) and os.WSTOPSIG(status) == signal.SIGSTOP:
                    self.suspended = self.handoff_suspended = True
                    return
                if os.WIFEXITED(status) or os.WIFSIGNALED(status):
                    self.process.returncode = os.waitstatus_to_exitcode(status)
                    self.exit_observed = True
                raise LoaderFailed("loader exited instead of entering the handoff barrier")
            time.sleep(0.01)
        raise ProbeError("loader stop observation timed out")

    def handoff(self):
        require(self.prompt_seen and self.echo_disabled and self.secret_writes == 0,
                "loader secret handoff was not admitted")
        self.suspend()
        payload = native.PASSWORD + b"\n"
        require(os.write(self.master, payload) == len(payload), "PTY secret handoff was incomplete")
        self.secret_writes = 1
        require(not life.pidfd_ready(self.pidfd, 0), "loader died during the handoff barrier")

    def complete(self):
        require(self.suspended, "loader did not reach its handoff barrier")
        life.pidfd_signal(self.pidfd, signal.SIGCONT)
        self.suspended = False
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.05)[0]:
                self.read_output()
            code = self.process.poll()
            if code is not None:
                self.exit_observed = life.pidfd_ready(self.pidfd, 0)
                require(self.exit_observed, "loader exit was not observed")
                if code != 0:
                    raise LoaderFailed("native loader exited unsuccessfully")
                return
        raise ProbeError("native loader completion timed out")

    def stop(self):
        if self.process is None:
            return
        if self.pidfd is not None:
            life.stop_agent(self.pidfd)
        elif self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=life.DEATH_TIMEOUT)
        self.exit_observed = self.pidfd is None or life.pidfd_ready(self.pidfd, 0)
        require(self.exit_observed, "loader shutdown was not observed")

    def close(self):
        require(self.process is None or self.exit_observed, "loader descriptors remain leased until exit")
        errors = []
        for name in ("master", "slave", "pidfd"):
            descriptor = getattr(self, name)
            setattr(self, name, None)
            if descriptor is not None:
                try:
                    os.close(descriptor)
                except OSError as error:
                    errors.append(error)
        if errors:
            raise errors[0]


class LoadingSession(session.Session):
    def __init__(self, writer, verifier, package, root, control):
        super().__init__(writer, verifier, package, root)
        self.control = control
        self.loader = OwnedLoader(self)
        self.last_seam = None
        self.resources_closed_after_exit = False
        self.master_transferred = False
        self.phase = "session-created"

    def _open_package(self):
        self.phase = "session-open-package"
        super()._open_package()

    def _launch_agent(self, *, isolate_terminal=False):
        self.phase = "session-launch-agent"
        super()._launch_agent(isolate_terminal=isolate_terminal)

    def stage(self, name):
        self.phase = name
        self.last_seam = name
        life.send_json(self.control, {"kind": "stage", "seam": name,
                                     "secret_writes": self.loader.secret_writes,
                                     "loader_suspended_at_handoff": self.loader.handoff_suspended,
                                     "loader_returncode": None if self.loader.process is None else self.loader.process.returncode,
                                     "loader_exit_observed": self.loader.exit_observed})
        command = receive(self.control)
        require(command in ({"command": "continue"}, {"command": "cancel"}, {"command": "loader-killed"}),
                "observer stage command was refused")
        if command["command"] == "cancel":
            raise Cancelled("startup cancelled at selected seam")
        if command["command"] == "loader-killed":
            require(life.pidfd_ready(self.loader.pidfd), "loader death trigger was not observed")
            self.loader.process.wait(timeout=life.DEATH_TIMEOUT)
            self.loader.exit_observed = True
            require(self.loader.process.returncode == -signal.SIGKILL, "loader failure was not SIGKILL")
            raise LoaderFailed("native loader killed at selected seam")

    def _load_agent(self):
        self.phase = "loader-start"
        self._validate()
        self.loader.start()
        self.phase = "transfer-master"
        send_fd(self.control, self.loader.master)
        self.master_transferred = True
        life.send_json(self.control, {"kind": "children", "agent_pid": self.agent.pid,
                                     "agent_start": self.agent_start, "loader_pid": self.loader.process.pid,
                                     "loader_start": self.loader.start_time,
                                     "socket": str(self.socket_path), "echo_disabled": self.loader.echo_disabled})
        self.stage("prompt")
        self.loader.handoff()
        self.stage("secret-handoff")
        self.loader.complete()
        self._validate()
        self.agent_loaded = True

    def _release(self):
        errors = []
        for stop in (self.loader.stop, self._stop_agent):
            try:
                stop()
            except BaseException as error:
                errors.append(error)
        if errors:
            raise ProbeError("native shutdown remains uncertain") from errors[0]
        self.loader.close()
        super()._release()
        self.resources_closed_after_exit = True


def worker(args):
    control = socket.socket(fileno=args.control_fd)
    value = LoadingSession(args.writer, args.verifier, args.package, args.root, control)
    outcome = "failed"
    primary = None
    try:
        receipt = value.start(before_ready=lambda current: current.stage("before-readiness"))
        value.last_seam = "ready"
        life.send_json(control, {"kind": "ready", "proof": receipt})
        require(receive(control) == {"command": "close"}, "ready close command was refused")
        value.lock()
        outcome = "ready_closed"
    except Cancelled as error:
        outcome = "cancelled"
        if not value.master_transferred:
            primary = error
    except LoaderFailed as error:
        outcome = "loader_failed_closed"
        if not value.master_transferred:
            primary = error
    except BaseException as error:
        outcome = "failed"
        primary = error
    try:
        try:
            if value.state != "CLOSED":
                value.lock()
        except BaseException as error:
            primary = primary or error
        if primary is not None:
            phase = value.loader.phase if value.phase == "loader-start" else value.phase
            if not value.master_transferred:
                control.sendall(b"E")
            life.send_json(control, {"kind": "worker-error", "diagnostic": safe_error(primary, phase)})
            return 1
        life.send_json(control, {"kind": "final", "outcome": outcome, "selected_seam": value.last_seam,
                                 "readiness_published": value.ready_published,
                                 "close_acknowledged": value.close_acknowledged,
                                 "secret_writes": value.loader.secret_writes,
                                 "prompt_echo_disabled": value.loader.echo_disabled,
                                 "loader_suspended_at_handoff": value.loader.handoff_suspended,
                                 "loader_returncode": None if value.loader.process is None else value.loader.process.returncode,
                                 "agent_returncode": None if value.agent is None else value.agent.returncode,
                                 "resources_closed_after_exit": value.resources_closed_after_exit})
        return 0 if outcome != "failed" else 1
    finally:
        control.close()


def selected_case(name):
    if name == "load-ready-close":
        return "ready", "close", "ready_closed"
    if name == "loader-death-at-prompt":
        return "prompt", "loader-kill", "loader_failed_closed"
    seam = "prompt" if name.endswith("at-prompt") else "secret-handoff" if name.endswith("after-secret-handoff") else "before-readiness"
    return seam, "supervisor-kill" if name.startswith("supervisor-kill") else "cancel", \
        "supervisor_killed" if name.startswith("supervisor-kill") else "cancelled"


def require_final(final, name, seam, expected):
    require(final.get("kind") == "final" and final.get("outcome") == expected
            and final.get("selected_seam") == seam, "case reached the wrong startup rejection gate")
    require(final.get("readiness_published") is (name == "load-ready-close")
            and final.get("close_acknowledged") is True
            and final.get("resources_closed_after_exit") is True, "startup closure was not acknowledged safely")


def cleanup_observer(bundle):
    global CLEANUP_UNCERTAIN
    CLEANUP_UNCERTAIN = True
    UNCERTAIN_BUNDLES.append(bundle)
    errors = []
    process = bundle["process"]
    safe = process is not None and bundle["agent_pidfd"] is not None and bundle["loader_pidfd"] is not None
    try:
        if process is not None and process.poll() is None:
            process.kill()
            process.wait(timeout=life.DEATH_TIMEOUT)
        safe = safe and process is not None and process.poll() is not None
        if bundle["parent_pidfd"] is not None:
            safe = life.pidfd_ready(bundle["parent_pidfd"], 0) and safe
    except BaseException as error:
        errors.append(error)
        safe = False
    for name in ("agent_pidfd", "loader_pidfd"):
        try:
            if bundle[name] is not None:
                life.stop_agent(bundle[name])
        except BaseException as error:
            errors.append(error)
            safe = False
    if not safe:
        raise ProbeError("observer could not confirm all native exits") from (errors[0] if errors else None)
    for name in ("connection", "control"):
        connection, bundle[name] = bundle[name], None
        if connection is not None:
            try:
                connection.close()
            except OSError as error:
                errors.append(error)
    for name in ("agent_pidfd", "loader_pidfd", "parent_pidfd", "pty_master"):
        descriptor, bundle[name] = bundle[name], None
        if descriptor is not None:
            try:
                os.close(descriptor)
            except OSError as error:
                errors.append(error)
    if errors:
        raise ProbeError("observer descriptor cleanup was incomplete") from errors[0]
    package = session.package_backend()
    package.cleanup_owned_tree(bundle["package"], bundle["package_identity"])
    package.cleanup_owned_tree(bundle["root"], bundle["root_identity"])
    UNCERTAIN_BUNDLES.remove(bundle)
    CLEANUP_UNCERTAIN = bool(UNCERTAIN_BUNDLES)


def finish_observer(bundle, primary, phase):
    if primary is not None and CURRENT_REPORT is not None:
        CURRENT_REPORT["primary_failure"] = safe_error(primary, phase)
    try:
        cleanup_observer(bundle)
    except BaseException as error:
        if CURRENT_REPORT is not None:
            CURRENT_REPORT["cleanup_failure"] = safe_error(error, "observer-cleanup")
        if primary is None:
            raise


def run_case(writer, verifier, store, root, name):
    global CURRENT_STAGE, CLEANUP_UNCERTAIN
    CURRENT_STAGE = name
    package = session.package_backend()
    case_root = Path(tempfile.mkdtemp(prefix="loader-case-", dir=root))
    case_root.chmod(0o700)
    root_identity = package.validate_safe_tree_root(case_root)
    (case_root / "home").mkdir(mode=0o700)
    package_path = store / name
    package.create_positive(writer, package_path, life.private_env(case_root))
    package_identity = package.validate_safe_tree_root(package_path)
    manifest, _ = package.read_manifest(package_path)
    parsed = native.parse_key(package_path / "key.openssh")
    challenge = native.challenge(parsed["public_key"], manifest["device_id"], manifest["credential_id"])
    message = native.canonical(challenge)
    comment = "vivac.device-key.v1:" + manifest["device_id"] + ":" + manifest["credential_id"]
    parent, child = socket.socketpair()
    process = None
    parent_pidfd = master = agent_pidfd = loader_pidfd = connection = None
    agent_pid = loader_pid = None
    seam, trigger, expected_outcome = selected_case(name)
    metadata = final = None
    pre_ready_signature = False
    alive_at_trigger = False
    observed_seam = None
    primary = None
    observer_phase = "observer-spawn-worker"
    try:
        process = subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), "--internal-worker", "--writer", str(writer),
             "--verifier", str(verifier), "--package", str(package_path), "--root", str(case_root),
             "--control-fd", str(child.fileno())], env=life.private_env(case_root),
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            pass_fds=(child.fileno(),), close_fds=True)
        child.close()
        parent_pidfd = os.pidfd_open(process.pid)
        observer_phase = "observer-await-master"
        master = receive_fd(parent)
        observer_phase = "observer-await-children"
        metadata = receive(parent)
        require(metadata.get("kind") == "children" and metadata.get("echo_disabled") is True,
                "native child metadata was refused")
        agent_pid, loader_pid = metadata["agent_pid"], metadata["loader_pid"]
        agent_pidfd = claim_child(agent_pid, metadata["agent_start"], process.pid)
        loader_pidfd = claim_child(loader_pid, metadata["loader_start"], process.pid)
        life.inspect_agent(agent_pid, metadata["agent_start"], shutil.which("ssh-agent", path=os.defpath), process.pid)
        life.inspect_agent(loader_pid, metadata["loader_start"], shutil.which("ssh-add", path=os.defpath), process.pid)
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(TIMEOUT)
        connection.connect(metadata["socket"])
        life.exchange(connection, b"\x0b")
        for current in ("prompt", "secret-handoff", "before-readiness"):
            observer_phase = "observer-await-" + current
            stage = receive(parent)
            require(stage.get("kind") == "stage" and stage.get("seam") == current,
                    "loader did not reach its selected startup seam")
            observed_seam = current
            if current == "secret-handoff":
                require(stage.get("secret_writes") == 1 and stage.get("loader_suspended_at_handoff") is True
                        and not life.pidfd_ready(loader_pidfd, 0), "loader handoff was not a live suspended seam")
            if current == "before-readiness":
                require(life.pidfd_ready(loader_pidfd, 0) and stage.get("loader_returncode") == 0
                        and stage.get("loader_exit_observed") is True,
                        "loader did not exit successfully before readiness seam")
                connection.close()
                connection, signature = life.open_signed_connection(
                    Path(metadata["socket"]), parsed["public_blob"], comment, message)
                native.verify(verifier, case_root, life.private_env(case_root), native.proof(challenge, signature),
                              "pre-ready-capability", True)
                pre_ready_signature = True
            if current == seam:
                break
            life.send_json(parent, {"command": "continue"})
        if seam == "ready":
            observer_phase = "observer-await-ready"
            ready = receive(parent)
            require(ready.get("kind") == "ready", "positive startup did not publish readiness")
            native.verify(verifier, case_root, life.private_env(case_root), ready["proof"], "readiness-independent", True)
            observed_seam = "ready"
        require(observed_seam == seam, "selected startup seam was not reached")
        alive_at_trigger = not life.pidfd_ready(loader_pidfd, 0)
        require(alive_at_trigger == (seam in ("prompt", "secret-handoff")), "loader liveness did not match selected seam")
        if trigger == "supervisor-kill":
            observer_phase = "observer-parent-death"
            process.kill()
            process.wait(timeout=life.DEATH_TIMEOUT)
            require(process.returncode == -signal.SIGKILL, "supervisor was not killed by SIGKILL")
            require(life.pidfd_ready(parent_pidfd) and life.pidfd_ready(loader_pidfd)
                    and life.pidfd_ready(agent_pidfd), "native children survived supervisor death")
            agent_status = life.reap_owned(agent_pid)
            require(agent_status is not None and os.WIFSIGNALED(agent_status)
                    and os.WTERMSIG(agent_status) == signal.SIGKILL, "orphan agent did not receive SIGKILL")
            loader_code = stage["loader_returncode"]
            if alive_at_trigger:
                loader_status = life.reap_owned(loader_pid)
                require(loader_status is not None and os.WIFSIGNALED(loader_status)
                        and os.WTERMSIG(loader_status) == signal.SIGKILL, "orphan loader did not receive SIGKILL")
                loader_code = -signal.SIGKILL
            final = {"readiness_published": False, "close_acknowledged": False,
                     "secret_writes": 0 if seam == "prompt" else 1,
                     "loader_returncode": loader_code, "agent_returncode": -signal.SIGKILL,
                     "resources_closed_after_exit": False}
        else:
            if trigger == "loader-kill":
                life.pidfd_signal(loader_pidfd, signal.SIGKILL)
                require(life.pidfd_ready(loader_pidfd), "loader kill was not observed")
            life.send_json(parent, {"command": "close" if trigger == "close" else "cancel" if trigger == "cancel" else "loader-killed"})
            observer_phase = "observer-await-final"
            final = receive(parent)
            require_final(final, name, seam, expected_outcome)
            require(life.pidfd_ready(loader_pidfd, 0) and life.pidfd_ready(agent_pidfd, 0),
                    "acknowledged closure preceded native exit")
            process.wait(timeout=TIMEOUT)
            require(process.returncode == 0, "supervisor failed after acknowledging closure")
        require(life.request_after_death(connection, parsed["public_blob"], message)
                in ("transport_refused", "eof", "refused"), "retained connection signed after loader closure")
        require(life.new_connection_after_death(Path(metadata["socket"]), parsed["public_blob"], message)
                in ("connect_refused", "transport_refused", "eof", "refused"), "new connection signed after loader closure")
        require(final["secret_writes"] == (0 if seam == "prompt" else 1), "case crossed the wrong secret-handoff boundary")
        return {"case": name, "selected_seam": seam, "selected_seam_reached": True,
                "trigger": trigger, "outcome": expected_outcome,
                "readiness_published": final["readiness_published"], "close_acknowledged": final["close_acknowledged"],
                "secret_writes": final["secret_writes"], "prompt_echo_disabled": True,
                "pty_master_retained": True, "loader_alive_at_trigger": alive_at_trigger,
                "loader_suspended_at_handoff": seam != "prompt", "loader_exit_observed": True,
                "agent_exit_observed": True, "supervisor_exit_observed": life.pidfd_ready(parent_pidfd, 0),
                "loader_returncode": final["loader_returncode"], "agent_returncode": final["agent_returncode"],
                "supervisor_returncode": process.returncode,
                "resources_closed_after_exit": final["resources_closed_after_exit"],
                "retained_connection_refused": True, "new_connection_refused": True,
                "pre_ready_signature_verified": pre_ready_signature, "processes_drained": True}
    except BaseException as error:
        primary = error
        raise
    finally:
        child.close()
        finish_observer({"process": process, "control": parent, "connection": connection,
                          "agent_pidfd": agent_pidfd, "loader_pidfd": loader_pidfd,
                          "parent_pidfd": parent_pidfd, "pty_master": master,
                          "package": package_path, "package_identity": package_identity,
                          "root": case_root, "root_identity": root_identity}, primary, observer_phase)


def run_probe(writer, verifier, store, *, diagnostic_startup=False):
    global CURRENT_REPORT
    report = {"version": 1, "status": "incomplete", "custody_accredited": False,
              "loader_lifecycle_tested": False, "ssh_disconnect_tested": False,
              "diagnostic_only": diagnostic_startup, "cases": [], "sentinel_survived": False}
    CURRENT_REPORT = report
    life.require_isolation()
    life.require_unprivileged()
    require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "observer is missing no_new_privs")
    report["resource_limits"] = life.require_resources(expected_script="loader_probe.py")
    session.require_store(store)
    life.prctl(life.PR_SET_CHILD_SUBREAPER, 1)
    package = session.package_backend()
    root = Path(tempfile.mkdtemp(prefix="vivac-loader-", dir="/tmp"))
    root.chmod(0o700)
    root_identity = package.validate_safe_tree_root(root)
    try:
        (root / "home").mkdir(mode=0o700)
        ssh_agent, ssh_add, comment, parsed, challenge, message, env = life.create_fixture(root, writer, verifier)
        sentinel_socket = root / "sentinel.sock"
        with life.owned_sentinel(ssh_agent, root, sentinel_socket) as (sentinel, pidfd):
            life.wait_socket(sentinel_socket, sentinel, sentinel.pid)
            code, _ = native.pty_run([ssh_add, "-k", str(root / "fixture-key")],
                                     life.private_env(root, sentinel_socket), [native.PASSWORD])
            require(code == 0, "sentinel credential load failed")
            for name in (("cancel-at-prompt",) if diagnostic_startup else CASES):
                report["cases"].append(run_case(writer, verifier, store, root, name))
                require(sentinel.poll() is None and not life.pidfd_ready(pidfd, 0), "sentinel was affected")
                signature = native.sign(sentinel_socket, parsed["public_blob"], message)
                native.verify(verifier, root, env, native.proof(challenge, signature), "sentinel-" + name, True)
            report["sentinel_survived"] = True
    finally:
        if not CLEANUP_UNCERTAIN:
            package.cleanup_owned_tree(root, root_identity)
    if diagnostic_startup:
        report["diagnostic_startup_completed"] = True
    else:
        report["loader_lifecycle_tested"] = True
        report["status"] = "observed"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--writer", type=Path)
    parser.add_argument("--verifier", type=Path)
    parser.add_argument("--store-root", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--internal-worker", action="store_true")
    parser.add_argument("--internal-loader-exec", action="store_true")
    parser.add_argument("--package", type=Path)
    parser.add_argument("--root", type=Path)
    parser.add_argument("--control-fd", type=int)
    parser.add_argument("--expected-parent", type=int)
    parser.add_argument("--slave-fd", type=int)
    parser.add_argument("--cipher-fd", type=int)
    parser.add_argument("--home")
    parser.add_argument("--agent-socket")
    parser.add_argument("--diagnostic-startup", action="store_true")
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                "this experiment requires an explicitly disposable Linux container")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "native process is missing no_new_privs")
        require(not args.diagnostic_startup or not (args.internal_worker or args.internal_loader_exec),
                "startup diagnosis is an observer-only operation")
        if args.internal_loader_exec:
            return loader_exec(args)
        if args.internal_worker:
            return worker(args)
        require(args.writer is not None and args.verifier is not None and args.store_root is not None
                and args.report is not None, "required experiment arguments are absent")
        session.require_report_directory(args.report)
        writer, verifier = args.writer.resolve(strict=True), args.verifier.resolve(strict=True)
        require(writer.is_file() and verifier.is_file() and os.access(writer, os.X_OK)
                and os.access(verifier, os.X_OK), "required helper executable is unavailable")
        result = run_probe(writer, verifier, args.store_root, diagnostic_startup=args.diagnostic_startup)
        life.private_write(args.report, json.dumps(result, sort_keys=True, indent=2).encode("ascii") + b"\n")
        print("Loader observations recorded; custody_accredited=false")
        return 1 if args.diagnostic_startup else 0
    except life.ParentChanged:
        return 78
    except Exception as error:
        failure = safe_error(error, CURRENT_STAGE)
    if args.report is not None:
        try:
            session.require_report_directory(args.report)
            result = dict(CURRENT_REPORT or {})
            result.update({"version": 1, "status": "incomplete", "custody_accredited": False,
                           "stage": CURRENT_STAGE, "failure": failure})
            life.private_write(args.report, json.dumps(result, sort_keys=True).encode("ascii") + b"\n")
        except (OSError, session.ProbeError):
            pass
    print("Loader probe failed; no result is accredited.", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
