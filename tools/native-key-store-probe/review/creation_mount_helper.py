"""Mount one selected disposable object in the target container's namespace."""
import argparse
import os
from pathlib import Path
import signal
import stat
import sys

import creation_mount_common as common


def validate_path_components(target):
    current = Path("/")
    for name in Path(target).parts[1:-1]:
        current /= name
        info = current.lstat()
        common.require(stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode),
                       "mount target ancestor is not a real directory")


def run(request):
    common.validate_request(request)
    namespaces = common.helper_same_namespace_checks()
    root_fd, mount_fd = common.open_mount_namespace_inputs()
    descriptor = None
    mounted_here = False
    try:
        common.join_target_mount_namespace(root_fd, mount_fd, namespaces["target_mount_namespace"])
        original_root = os.fstat(root_fd)
        named_root = os.stat("/", follow_symlinks=False)
        common.require((original_root.st_dev, original_root.st_ino) == (named_root.st_dev, named_root.st_ino),
                       "mount target root descriptor differs")
        target, kind, original = request["target"], request["kind"], request["original"]
        validate_path_components(target)
        descriptor = common.open_object(target, kind)
        before = common.snapshot(target, kind, descriptor)
        if request["operation"] == "mount":
            common.require(before == original, "mount target differs from its retained original")
            common.libc_mount(target, target)
            mounted_here = True
            bound = common.snapshot(target, kind)
            common.make_mount_private(target)
            after = common.snapshot(target, kind)
            common.require(after == bound, "making the selected mount private changed its object or mount identity")
            common.require_private_mount(target, after["mount_id"])
            common.require(common.snapshot(target, kind, descriptor) == original,
                           "mount changed the retained original object")
        else:
            common.require(common.same_object(before, original) and before["mount_id"] != original["mount_id"],
                           "unmount target is not the covered original object")
            common.require_private_mount(target, before["mount_id"])
            os.close(descriptor)
            descriptor = None
            common.libc_umount(target)
            after = common.snapshot(target, kind)
        acknowledgment = dict(version=1, operation=request["operation"], check=request["check"], token=request["token"],
            status="observed", before=before, after=after, namespaces=namespaces,
            setns_verified=True, private_mount_verified=True)
        common.validate_ack(acknowledgment, request)
        mounted_here = False
        return acknowledgment
    except BaseException:
        if mounted_here:
            try:
                common.libc_umount(request["target"])
            except BaseException:
                pass
        raise
    finally:
        if descriptor is not None:
            os.close(descriptor)
        os.close(mount_fd)
        os.close(root_fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", required=True)
    args = parser.parse_args()
    common.require(sys.platform == "linux" and os.environ.get("VIVAC_OPENSSH_DISPOSABLE") == "1",
                   "disposable Linux environment required")
    request = common.validate_request(common.decode_json(args.request))
    signal.signal(signal.SIGALRM, lambda _signum, _frame: (_ for _ in ()).throw(TimeoutError()))
    signal.alarm(5)
    try:
        acknowledgment = run(request)
    finally:
        signal.alarm(0)
    sys.stdout.buffer.write(common.canonical(acknowledgment) + b"\n")
    sys.stdout.buffer.flush()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        result = dict(status="refused", error=str(error) if isinstance(error, common.ProbeError) else type(error).__name__)
        code = error.errno if isinstance(error, (common.ProbeError, OSError)) else None
        if code is not None:
            result["errno"] = code
        sys.stderr.buffer.write(common.canonical(result) + b"\n")
        raise SystemExit(1)
