"""Guard ownership and outcome distinctions between real producer attempts."""
import unittest

import creation_draft_probe as draft
import creation_multi_probe as multi


def actor(index, *, credential=None, outcome="durable"):
    record = dict(version=2, installation_id="0" * 25 + "1", creation_id="0" * 25 + str(index),
                  device_id="0" * 25 + str(index + 1), credential_id=credential or "0" * 25 + str(index + 2),
                  public_key=("a" if index == 2 else "b") * 64, ciphertext_sha256="c" * 64,
                  manifest_sha256="d" * 64, ciphertext_len=512, manifest_len=322,
                  creation_profile_revision=1, secret_policy_revision=1)
    published = outcome == "durable"
    return dict(record_canonical=draft.canonical(record).decode("ascii"), helper_pid=40 + index,
                helper_sha256="e" * 64, publication_outcome=outcome,
                package_published=published, record_published=published,
                experimental_durable_publication_ack=published,
                creation_admission=False, custody_accredited=False, producer_origin_checked=False)


class RelationshipTests(unittest.TestCase):
    def pair(self, case):
        first = actor(2, outcome="unpublished" if case == "multi-contended" else "durable")
        credential = draft.parse_record(first["record_canonical"])["credential_id"]
        second = actor(6, credential=credential if case == "multi-contended" else None)
        return {"first": first, "second": second}

    def test_accepts_distinct_real_attempts_and_their_own_outcomes(self):
        for case in multi.CASES:
            actors = self.pair(case)
            self.assertEqual(multi.validate_relationships(actors, case), actors)

    def test_loser_cannot_claim_the_winners_publication_or_ack(self):
        for field, value in (("package_published", True), ("record_published", True),
                             ("experimental_durable_publication_ack", True), ("publication_outcome", "durable")):
            actors = self.pair("multi-contended")
            actors["first"][field] = value
            with self.subTest(field=field), self.assertRaises(Exception):
                multi.validate_relationships(actors, "multi-contended")

    def test_attempts_must_have_distinct_keys_creation_identity_and_processes(self):
        for case in multi.CASES:
            for field in ("creation_id", "device_id", "public_key"):
                actors = self.pair(case)
                first = draft.parse_record(actors["first"]["record_canonical"])
                second = draft.parse_record(actors["second"]["record_canonical"])
                second[field] = first[field]
                actors["second"]["record_canonical"] = draft.canonical(second).decode("ascii")
                with self.subTest(case=case, field=field), self.assertRaises(Exception):
                    multi.validate_relationships(actors, case)
            actors = self.pair(case)
            actors["second"]["helper_pid"] = actors["first"]["helper_pid"]
            with self.subTest(case=case, field="helper_pid"), self.assertRaises(Exception):
                multi.validate_relationships(actors, case)

    def test_installation_credential_and_source_relations_are_not_interchangeable(self):
        for case in multi.CASES:
            for field in ("installation_id", "credential_id"):
                actors = self.pair(case)
                second = draft.parse_record(actors["second"]["record_canonical"])
                first = draft.parse_record(actors["first"]["record_canonical"])
                second[field] = first[field] if case == "multi-unrelated" and field == "credential_id" else "0" * 25 + "9"
                actors["second"]["record_canonical"] = draft.canonical(second).decode("ascii")
                with self.subTest(case=case, field=field), self.assertRaises(Exception):
                    multi.validate_relationships(actors, case)
            actors = self.pair(case)
            actors["second"]["helper_sha256"] = "f" * 64
            with self.subTest(case=case, field="helper_sha256"), self.assertRaises(Exception):
                multi.validate_relationships(actors, case)

    def test_pair_rejects_authority_and_missing_or_aliased_roles(self):
        for field in ("creation_admission", "custody_accredited", "producer_origin_checked"):
            actors = self.pair("multi-unrelated")
            actors["second"][field] = True
            with self.subTest(field=field), self.assertRaises(Exception):
                multi.validate_relationships(actors, "multi-unrelated")
        for actors in ({}, {"first": actor(2)}, {"first": actor(2), "second": actor(2), "extra": actor(6)}):
            with self.subTest(roles=tuple(actors)), self.assertRaises(Exception):
                multi.validate_relationships(actors, "multi-unrelated")


if __name__ == "__main__":
    unittest.main()
