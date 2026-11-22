"""Observe disposable retained-child exit evidence without granting session authority."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys

import lifecycle_probe as life

CASES = ("alive-pending", "stopped-pending", "exit-before-reap", "exit-after-reap", "unknown-token-refused")
FIELDS = {"version", "case", "pending_observed", "stopped_observed", "stopped_pending_observed",
          "exit_observed", "reaped_before_observation", "consumed_rejected", "proof_revalidated",
          "unknown_token_refused", "native_exit_observed", "closure_authority", "session_activated"}


class ProbeError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProbeError(message)


def validate_case(name, value):
    require(name in CASES and type(value) is dict and set(value) == FIELDS,
            "child exit witness fields differ")
    require(type(value["version"]) is int and value["version"] == 1 and value["case"] == name,
            "child exit witness identity differs")
    require(all(type(value[field]) is bool for field in FIELDS - {"version", "case"}),
            "child exit witness evidence types differ")
    require(all(value[field] is True for field in ("pending_observed", "exit_observed", "consumed_rejected",
                                                 "proof_revalidated", "native_exit_observed")),
            "child exit witness is incomplete")
    require(value["stopped_observed"] is (name == "stopped-pending")
            and value["stopped_pending_observed"] is (name == "stopped-pending")
            and value["reaped_before_observation"] is (name == "exit-after-reap")
            and value["unknown_token_refused"] is (name == "unknown-token-refused"),
            "child exit witness ordering differs")
    require(value["closure_authority"] is False and value["session_activated"] is False,
            "child exit witness claims session authority")
    return value


def validate_report(value):
    require(type(value) is dict and set(value) == {"version", "status", "cases", "helper_sha256", "source_sha256",
                                                 "resource_limits", "uid", "closure_authority", "session_activated"},
            "child exit witness report fields differ")
    require(type(value["version"]) is int and value["version"] == 1 and value["status"] == "observed"
            and type(value["uid"]) is int and value["uid"] == 65534
            and value["closure_authority"] is False and value["session_activated"] is False,
            "child exit witness report identity differs")
    for field in ("helper_sha256", "source_sha256"):
        require(type(value[field]) is str and len(value[field]) == 64
                and all(character in "0123456789abcdef" for character in value[field]),
                "child exit witness hash differs")
    limits = value["resource_limits"]
    require(type(limits) is dict and set(limits) == {"memory_bytes", "swap_bytes", "pids", "cpu_quota",
                                                 "cpu_period", "temporary_bytes"}
            and all(type(item) is int for item in limits.values())
            and 0 < limits["memory_bytes"] <= 512 * 1024 * 1024 and limits["swap_bytes"] == 0
            and 0 < limits["pids"] <= 128 and 0 < limits["cpu_quota"] <= limits["cpu_period"]
            and 0 < limits["temporary_bytes"] <= 128 * 1024 * 1024,
            "child exit witness limits differ")
    require(type(value["cases"]) is dict and set(value["cases"]) == set(CASES),
            "child exit witness report cases differ")
    for name, evidence in value["cases"].items():
        validate_case(name, evidence)
    return value


def fingerprint(path):
    require(path.is_absolute() and not path.is_symlink() and path.is_file(), "child exit helper shape differs")
    with path.open("rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= 64 * 1024 * 1024,
                "child exit helper exceeds its bound")
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
        after = os.fstat(stream.fileno())
    fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
    require(all(getattr(before, field) == getattr(after, field) for field in fields),
            "child exit helper changed while inspected")
    return digest


def decode(payload):
    def unique(items):
        result = {}
        for key, value in items:
            require(key not in result, "child exit output has duplicate fields")
            result[key] = value
        return result
    require(0 < len(payload) <= 8192 and payload.endswith(b"\n"), "child exit output boundary differs")
    return json.loads(payload, object_pairs_hook=unique)


def observe_case(launcher, name, helper_hash):
    import select

    process = subprocess.Popen([str(launcher), "--exit-watch-case", name], stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, close_fds=True)
    descriptor = None
    try:
        descriptor = os.pidfd_open(process.pid)
        output, errors = process.communicate(timeout=20)
        require(process.returncode == 0 and not errors and len(output) <= 8192,
                "child exit helper failed or exceeded its output bound")
        poll = select.poll()
        poll.register(descriptor, select.POLLIN)
        events = poll.poll(0)
        require(len(events) == 1 and events[0][0] == descriptor
                and events[0][1] & select.POLLIN and not events[0][1] & (select.POLLERR | select.POLLNVAL),
                "child exit helper termination was not independently observed")
        result = validate_case(name, decode(output))
        require(fingerprint(launcher) == helper_hash, "child exit helper changed during execution")
        return result
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=2)
        if descriptor is not None:
            os.close(descriptor)
        for stream in (process.stdout, process.stderr):
            stream.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--launcher", type=Path, required=True)
    args = parser.parse_args()
    try:
        require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1"
                and os.getuid() == os.geteuid() == os.getgid() == os.getegid() == 65534,
                "child exit experiment requires disposable credentials")
        life.require_isolation()
        life.require_unprivileged()
        require(life.prctl(life.PR_GET_NO_NEW_PRIVS) == 1, "child exit experiment requires no-new-privileges")
        limits = life.require_resources(expected_script="exit_watch_probe.py")
        helper_hash, source_hash = fingerprint(args.launcher), fingerprint(Path(__file__).resolve())
        cases = {name: observe_case(args.launcher, name, helper_hash) for name in CASES}
        require(fingerprint(args.launcher) == helper_hash and fingerprint(Path(__file__).resolve()) == source_hash,
                "child exit helper or observer changed")
        report = dict(version=1, status="observed", cases=cases, helper_sha256=helper_hash,
                      source_sha256=source_hash, resource_limits=limits, uid=65534,
                      closure_authority=False, session_activated=False)
        validate_report(report)
        print(json.dumps(report, sort_keys=True))
        return 0
    except (ProbeError, life.ProbeError, OSError, ValueError, subprocess.SubprocessError) as error:
        frames = []
        frame = error.__traceback__
        while frame is not None:
            frames.append({"file": Path(frame.tb_frame.f_code.co_filename).name, "line": frame.tb_lineno})
            frame = frame.tb_next
        print(json.dumps({"status": "incomplete", "failure": {"kind": type(error).__name__, "frames": frames[-8:]}}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
