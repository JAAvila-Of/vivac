"""Run disposable administrative setup and an unprivileged attended draft."""
import argparse
from contextlib import contextmanager
import json
from pathlib import Path
import re
from types import SimpleNamespace

import creation_draft_probe as draft
import run_fixed_root_probe as fixed


def setup_case(case):
    if case not in draft.CASES:
        raise fixed.RunnerError("unknown draft experiment case")
    return "missing-fixed-root" if case == "missing-root" else "fixed-valid"


@contextmanager
def profile(writer, case):
    """Reuse the fixed-root controller's exact setup and configuration guards."""
    setup = setup_case(case)
    if not re.fullmatch(r"vivac-heap-review-build-[a-f0-9]{32}", writer):
        raise fixed.RunnerError("unexpected disposable writer volume")
    original = {name: getattr(fixed, name) for name in
                ("mount_specs", "create_arguments", "compare_phases", "probe", "command", "LIMITS")}
    reader_validated = False

    def mounts(helper, real, decoy, reports, phase, ignored):
        values = original["mount_specs"](helper, real, decoy, reports, phase, setup)
        if phase == "reader":
            for item in values:
                if item["Target"] == "/var/lib/vivac":
                    item["ReadOnly"] = False
            values.append(dict(Type="volume", Source=writer, Target="/draft-build", ReadOnly=True,
                               VolumeOptions={"NoCopy": True}))
            if case == "mount-packages":
                relative = "custody-v1/users/65534/state/packages"
                values.append(dict(Type="volume", Source=real, Target="/var/lib/vivac/" + relative,
                                   ReadOnly=False, VolumeOptions={"NoCopy": True, "Subpath": relative}))
        return values

    def arguments(image, helper, real, decoy, reports, phase, ignored, token=None):
        values = original["create_arguments"](image, helper, real, decoy, reports, phase, setup, token)
        if phase == "reader":
            values = values[:values.index("--entrypoint")] + [
                "--entrypoint", "/usr/bin/timeout", image, "--signal=TERM", "--kill-after=5s", "240s",
                "python3", "-B", "/probe/creation_draft_probe.py", "--case", case,
                "--writer", "/draft-build/writer189/debug/ssh-writer-graph-audit",
                "--expected-setup", "/reports/run/setup.json", "--report", "/reports/run/observations.json"]
        return values

    def validate(observation, phase, ignored=None):
        nonlocal reader_validated
        if phase == "setup":
            return original["probe"].validate_observation(observation, phase, setup)
        if phase != "reader":
            raise fixed.RunnerError("unexpected draft experiment phase")
        result = draft.validate_observation(observation, case)
        reader_validated = True
        return result

    def compare(administrative, observed, mutation=None):
        fixed.check(mutation is None)
        original["probe"].validate_observation(administrative, "setup", setup)
        draft.validate_observation(observed, case)
        expected = administrative["result"]
        fixed.check(observed["decoy_binding_sha256"] == expected["decoy_binding_sha256"])
        fixed.check(observed["real_binding_sha256"] == expected["real_binding_sha256"])
        if case not in draft.REFUSALS:
            fixed.check(observed["selected_installation_id"] == expected["real_installation_id"])
            fixed.check(observed["selected_installation_id"] != expected["decoy_installation_id"])

    def command(arguments, *args, **kwargs):
        if arguments[:3] == ["docker", "volume", "rm"] and not reader_validated:
            raise fixed.RunnerError("retain the isolated stores after an incomplete draft observation")
        return original["command"](arguments, *args, **kwargs)

    facade = SimpleNamespace(**vars(original["probe"]))
    facade.CASES = set(draft.CASES) | set(original["probe"].CASES)
    facade.MUTATIONS = set()
    facade.validate_observation = validate
    fixed.mount_specs = mounts
    fixed.create_arguments = arguments
    fixed.compare_phases = compare
    fixed.probe = facade
    fixed.command = command
    fixed.LIMITS = {**original["LIMITS"], "reader": (512, 128, 1, 128)}
    try:
        yield
    finally:
        for name, value in original.items():
            setattr(fixed, name, value)


def run(args):
    with profile(args.writer_build_volume, args.case):
        result = fixed.run(args)
    path = args.reports_dir / "runner.json"
    if path.exists():
        report = json.loads(path.read_text(encoding="utf-8"))
        report.update(experiment="rooted-creation-draft", writer_build_volume=args.writer_build_volume)
        report["candidate_rooted_draft_observed"] = report.pop("candidate_fixed_namespace_observed", False)
        path.write_text(json.dumps(report, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=sorted(draft.CASES), required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--launcher-build-volume", required=True)
    parser.add_argument("--writer-build-volume", required=True)
    parser.add_argument("--reports-dir", type=Path, required=True)
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
