"""Coordinate real disposable producers without threads or inferred authority."""
import copy
import os
from pathlib import Path
from types import SimpleNamespace

import creation_draft_probe as draft
import creation_reader_probe as joint
import creation_namespace_probe as namespaces

CASES = ("multi-unrelated", "multi-contended")
FLAGS = {"creation_admission", "custody_accredited", "producer_origin_checked"}
UNCERTAIN = []
require = draft.require


def validate_relationships(actors, case):
    require(case in CASES and type(actors) is dict and set(actors) == {"first", "second"},
            "multi-producer roles differ")
    first, second = actors["first"], actors["second"]
    require(type(first) is dict and type(second) is dict, "multi-producer actor type differs")
    left, right = (draft.parse_record(actor.get("record_canonical")) for actor in (first, second))
    require(left["installation_id"] == right["installation_id"]
            and all(left[field] != right[field] for field in ("creation_id", "device_id", "public_key")),
            "multi-producer identities are not independent")
    require((left["credential_id"] == right["credential_id"]) is (case == "multi-contended"),
            "multi-producer credential relationship differs")
    for actor in (first, second):
        require(type(actor.get("helper_pid")) is int and actor["helper_pid"] > 0
                and draft.fixed.hexadecimal(actor.get("helper_sha256"), 64), "multi-producer process or source differs")
        require(all(actor.get(field) is False for field in FLAGS), "multi-producer actor claims authority")
    require(first["helper_pid"] != second["helper_pid"] and first["helper_sha256"] == second["helper_sha256"],
            "multi-producer process or artifact was reused")
    for actor, published in ((first, case == "multi-unrelated"), (second, True)):
        require(actor.get("publication_outcome") == ("durable" if published else "unpublished"),
                "multi-producer attempt outcome differs")
        for field in ("package_published", "record_published", "experimental_durable_publication_ack"):
            require(actor.get(field) is published, "multi-producer attempt adopted another publication")
    return actors


def validate_observation(report, case):
    import creation_native_probe as producer
    require(type(report) is dict and case in CASES and report.get("case") == case
            and type(report.get("version")) is int and report["version"] == 1
            and report.get("experiment") == "rooted-native-creation" and report.get("status") == "observed",
            "multi-producer observation identity differs")
    require(all(report.get(field) is False for field in producer.FLAGS), "multi-producer observation claims authority")
    actors = validate_relationships(report.get("actors"), case)
    first, second = actors["first"], actors["second"]
    first_case = "publish-complete" if case == "multi-unrelated" else producer.CONTENDED_CASE
    producer.validate_observation(first, first_case, reader_required=case == "multi-unrelated")
    producer.validate_observation(second, "publish-complete")
    left, right = (draft.parse_record(actor["record_canonical"]) for actor in (first, second))
    require(report.get("selected_installation_id") == left["installation_id"], "multi-producer selected installation differs")
    for actor in (first, second):
        require(actor["real_binding_sha256"] == report.get("real_binding_sha256")
                and actor["decoy_binding_sha256"] == report.get("decoy_binding_sha256"),
                "multi-producer selected another installation binding")
        require(actor["helper_sha256"] == report.get("helper_sha256")
                and actor["observer_sha256"] == report.get("observer_sha256"), "multi-producer artifact provenance differs")
    second_reader = joint.validate_observation(second)
    require(second_reader["accepted"] is True and second_reader["revalidated"] is True,
            "winning publication was not independently readable")
    for field in ("process_exit_observed", "actor_exits_observed", "same_installation_verified",
                  "sources_unchanged", "helper_unchanged", "root_binding_unchanged"):
        require(report.get(field) is True, "multi-producer completion evidence missing")
    require(report.get("simultaneous_rename_race") is False, "controlled contention claims a simultaneous rename race")
    prior = namespaces.validate_summary(second.get("preserved_namespace"))
    if case == "multi-unrelated":
        require({entry["name"] for entry in prior["packages"]} == {left["credential_id"]}
                and {entry["name"] for entry in prior["admissions"]} == {left["credential_id"]},
                "unrelated producer did not retain the first publication")
        first_reader = joint.validate_observation(first)
        require(first_reader["unrelated_publication_observed"] is True and first_reader["revalidated"] is True
                and first.get("unrelated_producer") == second, "first snapshot did not survive the real second publication")
    else:
        stage = report.get("contended_stage_name")
        require(namespaces.package_name(stage) and stage.startswith(namespaces.PREFIX)
                and {entry["name"] for entry in prior["packages"]} == {stage} and not prior["admissions"],
                "winning producer did not retain the losing preparation")
        winner = namespaces.validate_summary(first.get("preserved_namespace"))
        require({entry["name"] for entry in winner["packages"]} == {right["credential_id"]}
                and {entry["name"] for entry in winner["admissions"]} == {right["credential_id"]},
                "losing producer did not preserve the winner")
        require(first.get("contended_stage_preserved") is True and first.get("final_slot_absent") is False
                and first.get("admissions_empty") is False and "fresh_reader" not in first,
                "losing attempt adopted or hid the winning commit")
    require(report.get("controlled_collision") is (case == "multi-contended")
            and report.get("first_stage_preserved") is (case == "multi-contended")
            and report.get("snapshot_survived_unrelated_publication") is (case == "multi-unrelated"),
            "multi-producer scenario outcome differs")
    return report


def _arguments(args, case, check="stable"):
    values = dict(vars(args))
    values.update(case=case, reader_check=check)
    return SimpleNamespace(**values)


def _runtime(parent, name):
    value = Path(parent) / name
    os.mkdir(value, 0o700)
    require(value.stat().st_uid == value.stat().st_gid == 65534 and value.stat().st_mode & 0o7777 == 0o700,
            "multi-producer private runtime differs")
    return value


def _actor(template, case):
    value = copy.deepcopy(template)
    value.update(case=case, status="incomplete", process_exit_observed=False)
    return value


def observe(args, report, bindings, provider, verifier, runtime):
    import creation_native_probe as producer
    require(args.case in CASES and args.reader_check == "stable", "multi-producer scenario selection differs")
    template = copy.deepcopy(report)
    first_case = "publish-complete" if args.case == "multi-unrelated" else producer.CONTENDED_CASE
    first_args = _arguments(args, first_case, "unrelated-publication" if args.case == "multi-unrelated" else "stable")
    first_runtime = _runtime(runtime, "first")
    first = _actor(template, first_case)
    second = None
    stage_name = None

    def second_producer(preserved, identifiers=None):
        nonlocal second
        second_args = _arguments(args, "publish-complete")
        second_runtime = _runtime(runtime, "second")
        second = _actor(template, "publish-complete")
        producer.observe(second_args, second, bindings, provider, verifier, second_runtime,
                         preserved=preserved, identifiers=identifiers)
        joint.observe(second_args, second, second_runtime)
        producer.finish_actor(second_args, second, bindings, provider, verifier)
        preserved.validate()
        return second

    if args.case == "multi-unrelated":
        producer.observe(first_args, first, bindings, provider, verifier, first_runtime)
        credential = draft.parse_record(first["record_canonical"])["credential_id"]

        def while_held():
            preserved = namespaces.PreservedNamespace(packages=(credential,), admissions=(credential,))
            try:
                result = second_producer(preserved)
                preserved.validate()
                return result
            except Exception:
                UNCERTAIN.append(preserved)
                raise
            finally:
                if second is not None and second.get("status") == "observed":
                    preserved.close()

        joint.observe(first_args, first, first_runtime, while_held=while_held)
        producer.finish_actor(first_args, first, bindings, provider, verifier)
    else:
        preserved_first = namespaces.PreservedNamespace()

        def before_publication(snapshot, active, retained):
            nonlocal stage_name
            stage_name = snapshot[0]
            credential = draft.parse_record(active["record_canonical"])["credential_id"]
            preserved_second = namespaces.PreservedNamespace(packages=(stage_name,))
            try:
                second_producer(preserved_second, (producer.native.identifier(), credential))
                retained.extend(packages=(credential,), admissions=(credential,))
                preserved_second.validate()
            except Exception:
                UNCERTAIN.append(preserved_second)
                raise
            finally:
                if second is not None and second.get("status") == "observed":
                    preserved_second.close()

        try:
            producer.observe(first_args, first, bindings, provider, verifier, first_runtime,
                             preserved=preserved_first, before_publication=before_publication)
            producer.finish_actor(first_args, first, bindings, provider, verifier, reader_required=False)
            preserved_first.validate()
        except Exception:
            UNCERTAIN.append(preserved_first)
            raise
        else:
            preserved_first.close()
    actors = {"first": first, "second": second}
    validate_relationships(actors, args.case)
    report.update(actors=actors, selected_installation_id=draft.parse_record(first["record_canonical"])["installation_id"],
                  process_exit_observed=True, actor_exits_observed=True, same_installation_verified=True,
                  simultaneous_rename_race=False, controlled_collision=args.case == "multi-contended",
                  first_stage_preserved=args.case == "multi-contended", contended_stage_name=stage_name,
                  snapshot_survived_unrelated_publication=args.case == "multi-unrelated")
