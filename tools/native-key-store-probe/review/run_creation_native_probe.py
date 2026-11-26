"""Run disposable setup and an unprivileged native creation experiment."""
import argparse
from contextlib import contextmanager
import json
from pathlib import Path

import creation_native_probe as native
import run_creation_draft_probe as base

fixed = base.fixed
PUBLIC_READER_CHECKS = ("stable", *native.joint.MUTATIONS, *native.joint.RESTORE_CHECKS)


@contextmanager
def profile(writer, case, reader_check="stable"):
    """Retain the existing exact guards while selecting the native observer."""
    if case not in native.CASES:
        raise fixed.RunnerError("unknown native creation experiment case")
    if reader_check not in PUBLIC_READER_CHECKS or (reader_check != "stable" and case != "publish-complete"):
        raise fixed.RunnerError("unsupported fresh reader experiment")
    original_draft = base.draft
    base.draft = native
    try:
        with base.profile(writer, case):
            original_arguments = fixed.create_arguments

            def arguments(image, helper, real, decoy, reports, phase, selected, token=None):
                values = original_arguments(image, helper, real, decoy, reports, phase, selected, token)
                if phase == "reader":
                    source = "/probe/creation_draft_probe.py"
                    fixed.check(values.count(source) == 1)
                    values[values.index(source)] = "/probe/creation_native_probe.py"
                    values += ["--reader", "/launcher-build/launcher189/debug/rust-launcher-probe",
                               "--reader-check", reader_check]
                return values

            fixed.create_arguments = arguments
            try:
                yield
            finally:
                fixed.create_arguments = original_arguments
    finally:
        base.draft = original_draft


def run(args):
    with profile(args.writer_build_volume, args.case, getattr(args, "reader_check", "stable")):
        result = fixed.run(args)
    path = args.reports_dir / "runner.json"
    if path.exists():
        report = json.loads(path.read_text(encoding="utf-8"))
        report.update(experiment="rooted-native-creation", writer_build_volume=args.writer_build_volume,
                      reader_check=getattr(args, "reader_check", "stable"))
        report["candidate_native_creation_observed"] = report.pop("candidate_fixed_namespace_observed", False)
        path.write_text(json.dumps(report, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=sorted(native.CASES), required=True)
    parser.add_argument("--reader-check", choices=PUBLIC_READER_CHECKS, default="stable")
    parser.add_argument("--image", required=True)
    parser.add_argument("--launcher-build-volume", required=True)
    parser.add_argument("--writer-build-volume", required=True)
    parser.add_argument("--reports-dir", type=Path, required=True)
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
