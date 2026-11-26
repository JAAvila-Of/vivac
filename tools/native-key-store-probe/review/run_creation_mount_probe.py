"""Exercise live self-bind rejection without granting the native actor capabilities."""
import argparse
from contextlib import contextmanager
import datetime
import io
import json
from pathlib import Path
import tarfile
import time
import uuid

import creation_mount_common as common
import run_creation_native_probe as base

fixed = base.fixed
CHECKS = ("writer-stage", "writer-key", "writer-manifest", "reader-package", "reader-key", "reader-manifest", "reader-record")
CAPS = ("SYS_ADMIN", "SYS_CHROOT", "SYS_PTRACE", "DAC_READ_SEARCH")


def helper_arguments(image, target, request):
    return ["docker", "create", "--name", "vivac-creation-mount-" + uuid.uuid4().hex,
            "--read-only", "--network", "none", "--cap-drop", "ALL",
            *[part for cap in CAPS for part in ("--cap-add", cap)],
            "--user", "0:0", "--security-opt", "no-new-privileges", "--pid=container:" + target,
            "--cpus=1", "--memory=128m", "--memory-swap=128m", "--pids-limit=32", "--ulimit", "core=0",
            "--log-driver", "json-file", "--log-opt", "max-size=1m", "--log-opt", "max-file=1",
            "--env", "PYTHONDONTWRITEBYTECODE=1",
            "--mount", "type=bind,source=" + fixed.REVIEW.as_posix() + ",target=/probe,readonly",
            "--entrypoint", "/usr/bin/timeout", image, "--kill-after=2s", "10s",
            "python3", "-B", "/probe/creation_mount_helper.py", "--request",
            json.dumps(request, sort_keys=True, separators=(",", ":"))]


def verify_helper(config, arguments, image, target):
    fixed.check(config["Image"] == image and config["Config"]["User"] == "0:0")
    offset = arguments.index("--entrypoint")
    fixed.check(config["Config"]["Entrypoint"] == ["/usr/bin/timeout"]
                and config["Config"]["Cmd"] == arguments[offset + 3:])
    host = config["HostConfig"]
    for key, expected in {"Memory": 128 * 1048576, "MemorySwap": 128 * 1048576,
                          "NanoCpus": 1000000000, "PidsLimit": 32, "ReadonlyRootfs": True,
                          "NetworkMode": "none", "Privileged": False, "PidMode": "container:" + target,
                          "IpcMode": "private", "SecurityOpt": ["no-new-privileges"], "CapDrop": ["ALL"],
                          "Ulimits": [{"Name": "core", "Hard": 0, "Soft": 0}],
                          "LogConfig": {"Type": "json-file", "Config": {"max-size": "1m", "max-file": "1"}}}.items():
        fixed.check(host.get(key) == expected)
    fixed.check(set(host.get("CapAdd") or []) == {"CAP_" + cap for cap in CAPS})
    fixed.check(not any(host.get(key) for key in ("Binds", "Devices", "DeviceRequests", "VolumesFrom", "Tmpfs", "UsernsMode")))
    fixed.check(len(host.get("Mounts") or []) == 1 and len(config["Mounts"]) == 1)
    source = host["Mounts"][0]
    fixed.check(source["Type"] == "bind" and Path(source["Source"]) == fixed.REVIEW
                and source["Target"] == "/probe" and source["ReadOnly"] is True)
    mount = config["Mounts"][0]
    fixed.check(mount["Type"] == "bind" and mount["Destination"] == "/probe"
                and mount["RW"] is False and mount["Propagation"] == "rprivate")
    environment = config["Config"]["Env"]
    fixed.check(len(environment) == 3 and dict(item.split("=", 1) for item in environment) == {
        "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "VIVAC_OPENSSH_DISPOSABLE": "1", "PYTHONDONTWRITEBYTECODE": "1"})


def parse_archive(raw, filename):
    fixed.check(len(raw) <= 131072)
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:") as archive:
        members = archive.getmembers()
        fixed.check(len(members) == 1 and members[0].isfile() and members[0].name == filename
                    and 0 < members[0].size <= 8192 and members[0].uid == members[0].gid == 65534
                    and members[0].mode == 0o600 and not members[0].linkname and members[0].sparse is None
                    and not any("sparse" in key.lower() for key in members[0].pax_headers))
        if "SCHILY.nlink" in members[0].pax_headers:
            fixed.check(members[0].pax_headers["SCHILY.nlink"] == "1")
        stream = archive.extractfile(members[0])
        fixed.check(stream is not None)
        value = base.native.draft.decode(stream.read(8193), 8192)
    common.validate_request(value)
    return value


@contextmanager
def profile(args, helper_records):
    with base.profile(args.writer_build_volume, args.case, "stable"):
        original_arguments, original_command = fixed.create_arguments, fixed.command
        target = None
        image = None
        serviced = {}
        servicing = False
        service_failed = False
        since = datetime.datetime.now(datetime.timezone.utc).isoformat()

        def events():
            found = fixed.rdp_events(since)
            if found:
                helper_records.append({"status": "incomplete", "rdp_events": found})
                raise fixed.RunnerError("new session errors interrupted mount helper")

        def arguments(*values, **kwargs):
            result = original_arguments(*values, **kwargs)
            if "/probe/creation_native_probe.py" in result:
                result += ["--mount-check", args.mount_check]
                index = result.index("--reader-check")
                result[index + 1] = args.reader_check
            return result

        def deliver(operation, ack):
            filename = operation + "-ack.json"
            destination = "/reports/run/" + filename
            temporary = "/reports/run/." + filename + "-" + ack["token"]
            payload = json.dumps(ack, sort_keys=True, separators=(",", ":"))
            fixed.check(len(payload) <= 32768)
            source = ("import os,sys; data=sys.argv[3].encode('ascii'); "
                      "fd=os.open(sys.argv[1],os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600); "
                      "assert os.write(fd,data)==len(data); os.fsync(fd); os.close(fd); "
                      "os.link(sys.argv[1],sys.argv[2]); os.unlink(sys.argv[1])")
            original_command(["docker", "exec", "--user", "65534:65534", target, "/usr/bin/python3",
                              "-I", "-S", "-B", "-c", source, temporary, destination, payload])

        def helper(request):
            events()
            record = {"operation": request["operation"], "status": "incomplete", "request": request}
            helper_records.append(record)
            directory = args.reports_dir / ("helper-" + request["operation"])
            directory.mkdir(exist_ok=False)
            container = None
            try:
                argv = helper_arguments(image, target, request)
                container = original_command(argv).decode("ascii").strip()
                record["container_id"] = container
                config = json.loads(original_command(["docker", "inspect", container]))[0]
                (directory / "config.json").write_text(json.dumps(config, sort_keys=True, indent=2), encoding="utf-8")
                verify_helper(config, argv, image, target)
                record["limits_verified"] = True
                original_command(["docker", "start", container])
                deadline = time.monotonic() + 11
                while True:
                    events()
                    state = json.loads(original_command(["docker", "inspect", "--format", "{{json .State}}", container]))
                    if not state["Running"]:
                        record["final_state"] = state
                        fixed.check(state["ExitCode"] == 0 and state["OOMKilled"] is False)
                        break
                    fixed.check(time.monotonic() < deadline)
                    time.sleep(.1)
                raw = original_command(["docker", "logs", container], merge_stderr=True)
                (directory / "output.log").write_bytes(raw)
                ack = base.native.draft.decode(raw.strip(), 32768)
                common.validate_ack(ack, request)
                record.update(status="observed", acknowledgement=ack)
                deliver(request["operation"], ack)
            finally:
                if container is not None:
                    state = json.loads(original_command(["docker", "inspect", "--format", "{{json .State}}", container]))
                    if state["Running"]:
                        original_command(["docker", "stop", "--time", "1", container])
                    record["cleanup_final_state"] = json.loads(original_command(["docker", "inspect", "--format", "{{json .State}}", container]))
                    (directory / "final-output.log").write_bytes(original_command(["docker", "logs", container], merge_stderr=True))
                    original_command(["docker", "rm", container])
                    record["removed"] = True

        def service():
            nonlocal servicing, service_failed
            if servicing or service_failed:
                return
            servicing = True
            try:
                operation = "mount" if "mount" not in serviced else "unmount"
                if operation in serviced:
                    return
                filename = operation + "-request.json"
                raw = original_command(["docker", "cp", target + ":/reports/run/" + filename, "-"], optional=True)
                if raw is None:
                    return
                request = parse_archive(raw, filename)
                fixed.check(request["operation"] == operation and request["check"] == args.mount_check)
                if operation == "unmount":
                    fixed.check(all(request[key] == serviced["mount"][key] for key in
                                    ("check", "target", "kind", "token", "original")))
                helper(request)
                serviced[operation] = request
            except BaseException:
                service_failed = True
                raise
            finally:
                servicing = False

        def command(values, *more, **kwargs):
            nonlocal target, image
            if values[:3] == ["docker", "volume", "rm"]:
                fixed.check(set(serviced) == {"mount", "unmount"})
            result = original_command(values, *more, **kwargs)
            if values[:2] == ["docker", "create"] and "/probe/creation_native_probe.py" in values:
                fixed.check(target is None)
                target = result.decode("ascii").strip()
                image = values[values.index("--entrypoint") + 2]
            if target is not None and values[:4] == ["docker", "inspect", "--format", "{{json .State}}"] and values[-1] == target:
                if json.loads(result)["Running"]:
                    service()
            return result

        fixed.create_arguments, fixed.command = arguments, command
        try:
            yield
        finally:
            fixed.create_arguments, fixed.command = original_arguments, original_command


def run(args):
    helpers = []
    try:
        with profile(args, helpers):
            result = fixed.run(args)
    finally:
        path = args.reports_dir / "runner.json"
        if path.exists():
            report = json.loads(path.read_text(encoding="utf-8"))
            report.update(experiment="rooted-live-mount", mount_check=args.mount_check,
                          reader_check=args.reader_check, mount_helpers=helpers)
            report["candidate_live_mount_observed"] = report.pop("candidate_fixed_namespace_observed", False)
            path.write_text(json.dumps(report, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mount-check", choices=CHECKS, required=True)
    for flag in ("image", "launcher-build-volume", "writer-build-volume"):
        parser.add_argument("--" + flag, required=True)
    parser.add_argument("--reports-dir", type=Path, required=True)
    args = parser.parse_args()
    args.case = "publish-invalidate-before-package" if args.mount_check.startswith("writer-") else "publish-complete"
    args.reader_check = "stable" if args.mount_check.startswith("writer-") else args.mount_check.replace("reader-", "mount-", 1)
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
