use super::super::tests as proposal_tests;
use super::*;
use ed25519_dalek::Signer;
use serde_json::{json, Value as Json};

const ORGANIZATION: &str = "00000000000000000000000001";
const OPERATION: &str = "00000000000000000000000005";
const PRINCIPAL: &str = "00000000000000000000000002";
const AUTHORITY: &str = "00000000000000000000000040";

fn organization_signing() -> ed25519_dalek::SigningKey {
    // RFC 8032 section 7.1, test 2; public test material only.
    ed25519_dalek::SigningKey::from_bytes(
        &crate::enrollment_proof::decode_hex::<32>(
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        )
        .unwrap(),
    )
}

fn input(shared_key: bool) -> Json {
    let mut value = proposal_tests::input();
    let key = if shared_key {
        proposal_tests::signing()
    } else {
        organization_signing()
    };
    value["version"] = json!(2);
    value["profile"] = json!(ACCEPTANCE_PROFILE);
    value["acceptance"] = json!({
        "organization_authority_id": if shared_key { PRINCIPAL } else { AUTHORITY },
        "organization_authority_public_key": hex(&key.verifying_key().to_bytes()),
        "principal_acceptance": "candidate-signing-key"
    });
    value
}

fn parse(value: &Json) -> RegistrationResult<Draft> {
    Draft::parse_using(
        &serde_json::to_vec(value).unwrap(),
        proposal_tests::runtime(),
    )
}

fn ready(shared_key: bool) -> (RegistrationVerifier, ReadyProposal) {
    let draft = parse(&input(shared_key)).unwrap();
    let (verifier, registration) = proposal_tests::registration_for(&draft);
    let ready = verifier
        .prepare_proposal_using(draft, registration, proposal_tests::confirmation_runtime())
        .unwrap();
    (verifier, ready)
}

fn expectations(proposal: [u8; 32], shared_key: bool) -> FixtureExpectations {
    let organization_key = if shared_key {
        proposal_tests::signing()
    } else {
        organization_signing()
    };
    FixtureExpectations::new_fixture(
        ORGANIZATION,
        OPERATION,
        proposal,
        FixtureSigner::new(
            if shared_key { PRINCIPAL } else { AUTHORITY },
            organization_key.verifying_key().to_bytes(),
        )
        .unwrap(),
        FixtureSigner::new(
            PRINCIPAL,
            proposal_tests::signing().verifying_key().to_bytes(),
        )
        .unwrap(),
    )
    .unwrap()
}

fn pending(
    shared_key: bool,
) -> (
    RegistrationVerifier,
    FixtureAcceptanceRequest,
    PendingFixtureAcceptances,
) {
    let (verifier, ready) = ready(shared_key);
    let expected = expectations(ready.commitment(), shared_key);
    let (request, pending) = verifier
        .begin_fixture_acceptances_using(ready, expected, proposal_tests::confirmation_runtime())
        .unwrap();
    (verifier, request, pending)
}

fn signatures(request: &FixtureAcceptanceRequest, shared_key: bool) -> ([u8; 64], [u8; 64]) {
    let organization_key = if shared_key {
        proposal_tests::signing()
    } else {
        organization_signing()
    };
    (
        organization_key
            .sign(&decode(&request.organization_message, 512).unwrap())
            .to_bytes(),
        proposal_tests::signing()
            .sign(&decode(&request.principal_message, 512).unwrap())
            .to_bytes(),
    )
}

fn verified() -> (RegistrationVerifier, FixtureVerifiedReadyProposal) {
    let (verifier, request, pending) = pending(false);
    let (organization, principal) = signatures(&request, false);
    let verified = verifier
        .finish_fixture_acceptances_using(
            pending,
            &organization,
            &principal,
            proposal_tests::confirmation_runtime(),
        )
        .unwrap();
    (verifier, verified)
}

fn expired() -> RegistrationResult<u64> {
    Ok(1_600_000_300)
}
fn rollback() -> RegistrationResult<u64> {
    Ok(1_600_000_003)
}
fn expired_monotonic() -> Instant {
    Instant::now() + Duration::from_secs(1000)
}

#[test]
fn version_one_preserves_bytes_and_refuses_any_acceptance_field() {
    let value = proposal_tests::input();
    let draft = parse(&value).unwrap();
    assert_eq!(draft.commitment(), hash(DRAFT_DOMAIN, draft.canonical()));
    assert!(serde_json::from_slice::<Json>(draft.canonical())
        .unwrap()
        .get("acceptance")
        .is_none());
    for acceptance in [
        Json::Null,
        input(false)["acceptance"].clone(),
        json!({}),
        json!(false),
    ] {
        let mut value = value.clone();
        value["acceptance"] = acceptance;
        assert!(parse(&value).is_err());
    }
}

#[test]
fn version_two_requires_exact_acceptance_declarations_and_profile_pair() {
    let original = input(false);
    assert!(parse(&original).is_ok());
    for field in [
        "organization_authority_id",
        "organization_authority_public_key",
        "principal_acceptance",
    ] {
        let mut value = original.clone();
        value["acceptance"].as_object_mut().unwrap().remove(field);
        assert!(parse(&value).is_err());
    }
    let mut missing = original.clone();
    missing.as_object_mut().unwrap().remove("acceptance");
    assert!(parse(&missing).is_err());
    for acceptance in [
        Json::Null,
        json!(false),
        json!([]),
        json!({"approved":true}),
    ] {
        let mut value = original.clone();
        value["acceptance"] = acceptance;
        assert!(parse(&value).is_err());
    }
    for (field, invalid) in [
        ("organization_authority_id", json!("not-an-identifier")),
        ("organization_authority_public_key", json!("00".repeat(32))),
        ("organization_authority_public_key", json!("11")),
        (
            "organization_authority_public_key",
            json!(hex(&organization_signing().verifying_key().to_bytes()).to_uppercase()),
        ),
        ("principal_acceptance", json!("approved")),
        ("approved", json!(true)),
    ] {
        let mut value = original.clone();
        value["acceptance"][field] = invalid;
        assert!(parse(&value).is_err());
    }
    for (version, profile) in [
        (1, ACCEPTANCE_PROFILE),
        (2, PROFILE),
        (3, ACCEPTANCE_PROFILE),
    ] {
        let mut value = original.clone();
        value["version"] = json!(version);
        value["profile"] = json!(profile);
        assert!(parse(&value).is_err());
    }
    let raw = serde_json::to_string(&original).unwrap();
    let duplicate = raw.replacen(
        "\"principal_acceptance\":",
        "\"principal_acceptance\":\"candidate-signing-key\",\"principal_acceptance\":",
        1,
    );
    assert!(Draft::parse_using(duplicate.as_bytes(), proposal_tests::runtime()).is_err());
    let duplicate = raw.replacen("\"acceptance\":", "\"acceptance\":null,\"acceptance\":", 1);
    assert!(Draft::parse_using(duplicate.as_bytes(), proposal_tests::runtime()).is_err());
}

#[test]
fn signers_and_fixture_expectations_have_no_weak_or_invalid_identity_defaults() {
    assert!(
        FixtureSigner::new("invalid", organization_signing().verifying_key().to_bytes()).is_err()
    );
    assert!(FixtureSigner::new(AUTHORITY, [0; 32]).is_err());
    for (organization, operation) in [("invalid", OPERATION), (ORGANIZATION, "invalid")] {
        assert!(FixtureExpectations::new_fixture(
            organization,
            operation,
            [1; 32],
            FixtureSigner::new(AUTHORITY, organization_signing().verifying_key().to_bytes())
                .unwrap(),
            FixtureSigner::new(
                PRINCIPAL,
                proposal_tests::signing().verifying_key().to_bytes()
            )
            .unwrap()
        )
        .is_err());
    }
}

#[test]
fn all_independent_fixture_expectation_mismatches_refuse_before_messages() {
    for field in 0..7 {
        let (verifier, ready) = ready(false);
        let mut expected = expectations(ready.commitment(), false);
        match field {
            0 => expected.organization = "00000000000000000000000009".into(),
            1 => expected.operation = "00000000000000000000000009".into(),
            2 => expected.proposal[0] ^= 1,
            3 => expected.organization_signer.id = "00000000000000000000000009".into(),
            4 => {
                expected.organization_signer.key =
                    proposal_tests::signing().verifying_key().to_bytes()
            }
            5 => expected.principal_signer.id = "00000000000000000000000009".into(),
            6 => expected.principal_signer.key = organization_signing().verifying_key().to_bytes(),
            _ => unreachable!(),
        }
        assert!(verifier
            .begin_fixture_acceptances_using(
                ready,
                expected,
                proposal_tests::confirmation_runtime()
            )
            .is_err());
    }
    let draft = parse(&proposal_tests::input()).unwrap();
    let (verifier, registration) = proposal_tests::registration_for(&draft);
    let ready = verifier
        .prepare_proposal_using(draft, registration, proposal_tests::confirmation_runtime())
        .unwrap();
    let expected = expectations(ready.commitment(), false);
    assert!(verifier
        .begin_fixture_acceptances_using(ready, expected, proposal_tests::confirmation_runtime())
        .is_err());
}

#[test]
fn changed_authority_or_key_changes_draft_and_refuses_original_registration() {
    let original = parse(&input(false)).unwrap();
    let original_commitment = original.commitment();
    for field in 0..2 {
        let (_, registration) = proposal_tests::registration_for(&original);
        let mut value = input(false);
        if field == 0 {
            value["acceptance"]["organization_authority_id"] = json!("00000000000000000000000041");
        } else {
            value["acceptance"]["organization_authority_public_key"] =
                json!(hex(&proposal_tests::signing().verifying_key().to_bytes()));
        }
        let changed = parse(&value).unwrap();
        assert_ne!(changed.commitment(), original_commitment);
        let verifier = RegistrationVerifier::new("vivac.test", "https://vivac.test").unwrap();
        assert!(verifier
            .prepare_proposal_using(
                changed,
                registration,
                proposal_tests::confirmation_runtime()
            )
            .is_err());
    }
}

#[test]
fn two_role_signatures_remain_distinct_even_for_an_explicitly_shared_key() {
    for shared in [false, true] {
        let (verifier, request, pending) = pending(shared);
        let (organization, principal) = signatures(&request, shared);
        assert_ne!(request.organization_message, request.principal_message);
        assert_ne!(organization, principal);
        if shared {
            let organization_message = decode(&request.organization_message, 512).unwrap();
            let principal_message = decode(&request.principal_message, 512).unwrap();
            assert_eq!(
                organization_message
                    .strip_prefix(ORGANIZATION_DOMAIN)
                    .unwrap(),
                principal_message.strip_prefix(PRINCIPAL_DOMAIN).unwrap()
            );
        }
        assert!(verifier
            .finish_fixture_acceptances_using(
                pending,
                &principal,
                &organization,
                proposal_tests::confirmation_runtime()
            )
            .is_err());
        let (verifier, request, pending) = self::pending(shared);
        let (organization, principal) = signatures(&request, shared);
        assert!(verifier
            .finish_fixture_acceptances_using(
                pending,
                &organization,
                &principal,
                proposal_tests::confirmation_runtime()
            )
            .is_ok());
    }
}

#[test]
fn malformed_missing_wrong_key_and_other_purpose_signatures_refuse_without_partial_output() {
    for fault in 0..10 {
        let (verifier, request, pending) = pending(false);
        let (organization, principal) = signatures(&request, false);
        let mut organization = organization.to_vec();
        let mut principal = principal.to_vec();
        match fault {
            0 => organization.clear(),
            1 => principal.clear(),
            2 => organization.push(0),
            3 => {
                principal.pop();
            }
            4 => organization[0] ^= 1,
            5 => principal[0] ^= 1,
            6 => {
                organization = proposal_tests::signing()
                    .sign(&decode(&request.organization_message, 512).unwrap())
                    .to_bytes()
                    .to_vec()
            }
            7 => {
                principal = organization_signing()
                    .sign(&decode(&request.principal_message, 512).unwrap())
                    .to_bytes()
                    .to_vec()
            }
            8 => {
                organization = crate::enrollment_proof::decode_hex::<64>(
                    &pending.ready.draft.proof().signature,
                )
                .unwrap()
                .to_vec()
            }
            9 => {
                let digest = confirmation_digest(
                    &pending.ready.reserved.registration,
                    pending.ready.draft.proof(),
                    pending.ready.binding,
                )
                .unwrap();
                principal = proposal_tests::signing()
                    .sign(&signing_message(&digest))
                    .to_bytes()
                    .to_vec();
            }
            _ => unreachable!(),
        }
        assert!(verifier
            .finish_fixture_acceptances_using(
                pending,
                &organization,
                &principal,
                proposal_tests::confirmation_runtime()
            )
            .is_err());
    }
}

#[test]
fn fixture_pending_and_verified_evidence_cannot_be_rebound() {
    for fault in 0..5 {
        let (verifier, request, mut pending) = pending(false);
        let (organization, principal) = signatures(&request, false);
        match fault {
            0 => pending.ready.packet.push(0),
            1 => pending.ready.binding.context.nonce[0] ^= 1,
            2 => pending.organization_message[0] ^= 1,
            3 => pending.expectations.proposal[0] ^= 1,
            4 => pending.principal_message[0] ^= 1,
            _ => unreachable!(),
        }
        assert!(verifier
            .finish_fixture_acceptances_using(
                pending,
                &organization,
                &principal,
                proposal_tests::confirmation_runtime()
            )
            .is_err());
    }
    for fault in 0..4 {
        let (verifier, mut verified) = verified();
        match fault {
            0 => verified.evidence.organization_signature[0] ^= 1,
            1 => verified.evidence.principal_message[0] ^= 1,
            2 => {
                verified.evidence.expectations.organization_signer.key =
                    proposal_tests::signing().verifying_key().to_bytes()
            }
            3 => verified.ready.packet.push(0),
            _ => unreachable!(),
        }
        assert!(verifier
            .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
            .is_err());
    }
}

#[test]
fn complete_owned_fixture_flow_retains_all_four_evidences_without_authority() {
    let (verifier, request, pending) = pending(false);
    let packet = pending.packet().to_vec();
    let commitment = pending.commitment();
    let (organization, principal) = signatures(&request, false);
    let verified = verifier
        .finish_fixture_acceptances_using(
            pending,
            &organization,
            &principal,
            proposal_tests::confirmation_runtime(),
        )
        .unwrap();
    assert_eq!(verified.packet(), packet);
    assert_eq!(verified.commitment(), commitment);
    let (assertion_request, pending) = verifier
        .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
        .unwrap();
    assert_eq!(pending.packet(), packet);
    assert_eq!(pending.commitment(), commitment);
    let (device, assertion) = proposal_tests::evidence(&assertion_request);
    let complete = verifier
        .finish_fixture_proposal_using(
            pending,
            &device,
            &assertion,
            proposal_tests::confirmation_runtime(),
        )
        .unwrap();
    assert_eq!(complete.packet(), packet);
    assert_eq!(complete.possession().binding().proposal, commitment);
    assert_eq!(complete.possession().device_signature(), device);
    assert_eq!(complete.possession().assertion(), assertion);
    assert_eq!(complete.possession().counter(), 10);
    assert_eq!(
        complete.acceptances().organization_signature(),
        organization
    );
    assert_eq!(complete.acceptances().principal_signature(), principal);
    assert_eq!(
        complete.acceptances().organization_message(),
        decode(&request.organization_message, 512).unwrap()
    );
    assert_eq!(
        complete.acceptances().principal_message(),
        decode(&request.principal_message, 512).unwrap()
    );
    assert_eq!(
        complete.acceptances().expectations.organization_signer.id,
        AUTHORITY
    );
    assert_eq!(
        complete.acceptances().expectations.principal_signer.id,
        PRINCIPAL
    );
}

#[test]
fn changing_accepting_authority_invalidates_both_acceptances_and_both_possessions() {
    let (verifier, old_request, old_pending) = pending(false);
    let (old_organization, old_principal) = signatures(&old_request, false);
    let old_verified = verifier
        .finish_fixture_acceptances_using(
            old_pending,
            &old_organization,
            &old_principal,
            proposal_tests::confirmation_runtime(),
        )
        .unwrap();
    let (old_assertion_request, old_pending) = verifier
        .start_fixture_proposal_using(old_verified, proposal_tests::confirmation_runtime())
        .unwrap();
    let (old_device, old_assertion) = proposal_tests::evidence(&old_assertion_request);
    let old_commitment = old_pending.commitment();
    drop(old_pending);
    for stale in 0..4 {
        let mut value = input(false);
        let changed_authority = "00000000000000000000000041";
        value["acceptance"]["organization_authority_id"] = json!(changed_authority);
        let draft = parse(&value).unwrap();
        let (verifier, registration) = proposal_tests::registration_for(&draft);
        let ready = verifier
            .prepare_proposal_using(draft, registration, proposal_tests::confirmation_runtime())
            .unwrap();
        assert_ne!(ready.commitment(), old_commitment);
        let expected = FixtureExpectations::new_fixture(
            ORGANIZATION,
            OPERATION,
            ready.commitment(),
            FixtureSigner::new(
                changed_authority,
                organization_signing().verifying_key().to_bytes(),
            )
            .unwrap(),
            FixtureSigner::new(
                PRINCIPAL,
                proposal_tests::signing().verifying_key().to_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
        let (request, pending) = verifier
            .begin_fixture_acceptances_using(
                ready,
                expected,
                proposal_tests::confirmation_runtime(),
            )
            .unwrap();
        let (organization, principal) = signatures(&request, false);
        if stale < 2 {
            let organization = if stale == 0 {
                old_organization
            } else {
                organization
            };
            let principal = if stale == 1 { old_principal } else { principal };
            assert!(verifier
                .finish_fixture_acceptances_using(
                    pending,
                    &organization,
                    &principal,
                    proposal_tests::confirmation_runtime()
                )
                .is_err());
        } else {
            let verified = verifier
                .finish_fixture_acceptances_using(
                    pending,
                    &organization,
                    &principal,
                    proposal_tests::confirmation_runtime(),
                )
                .unwrap();
            let (request, pending) = verifier
                .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
                .unwrap();
            let (device, assertion) = proposal_tests::evidence(&request);
            let device = if stale == 2 { old_device } else { device };
            let assertion = if stale == 3 {
                old_assertion.as_slice()
            } else {
                assertion.as_slice()
            };
            assert!(verifier
                .finish_fixture_proposal_using(
                    pending,
                    &device,
                    assertion,
                    proposal_tests::confirmation_runtime()
                )
                .is_err());
        }
    }
}

#[test]
fn original_time_and_monotonic_limits_apply_at_every_fixture_stage() {
    for clock in [expired as fn() -> RegistrationResult<u64>, rollback] {
        let runtime = Runtime {
            clock,
            ..proposal_tests::confirmation_runtime()
        };
        let (verifier, ready) = ready(false);
        let expected = expectations(ready.commitment(), false);
        assert!(verifier
            .begin_fixture_acceptances_using(ready, expected, runtime)
            .is_err());
        let (verifier, request, pending) = pending(false);
        let (organization, principal) = signatures(&request, false);
        assert!(verifier
            .finish_fixture_acceptances_using(pending, &organization, &principal, runtime)
            .is_err());
        let (verifier, verified) = self::verified();
        assert!(verifier
            .start_fixture_proposal_using(verified, runtime)
            .is_err());
        let (verifier, verified) = self::verified();
        let (request, pending) = verifier
            .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
            .unwrap();
        let (device, assertion) = proposal_tests::evidence(&request);
        assert!(verifier
            .finish_fixture_proposal_using(pending, &device, &assertion, runtime)
            .is_err());
    }
    let runtime = Runtime {
        monotonic: expired_monotonic,
        ..proposal_tests::confirmation_runtime()
    };
    let (verifier, ready) = ready(false);
    let expected = expectations(ready.commitment(), false);
    assert!(verifier
        .begin_fixture_acceptances_using(ready, expected, runtime)
        .is_err());
    let (verifier, request, pending) = pending(false);
    let (organization, principal) = signatures(&request, false);
    assert!(verifier
        .finish_fixture_acceptances_using(pending, &organization, &principal, runtime)
        .is_err());
    let (verifier, verified) = self::verified();
    assert!(verifier
        .start_fixture_proposal_using(verified, runtime)
        .is_err());
    let (verifier, verified) = self::verified();
    let (request, pending) = verifier
        .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
        .unwrap();
    let (device, assertion) = proposal_tests::evidence(&request);
    assert!(verifier
        .finish_fixture_proposal_using(pending, &device, &assertion, runtime)
        .is_err());
}

#[test]
fn acceptance_verification_rechecks_time_after_both_signatures() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static READS: AtomicUsize = AtomicUsize::new(0);
    fn advancing_clock() -> RegistrationResult<u64> {
        if READS.fetch_add(1, Ordering::SeqCst) >= 3 {
            expired()
        } else {
            Ok(1_600_000_004)
        }
    }
    let (verifier, request, pending) = pending(false);
    let (organization, principal) = signatures(&request, false);
    READS.store(0, Ordering::SeqCst);
    let runtime = Runtime {
        clock: advancing_clock,
        ..proposal_tests::confirmation_runtime()
    };
    assert!(verifier
        .finish_fixture_acceptances_using(pending, &organization, &principal, runtime)
        .is_err());
    assert!(READS.load(Ordering::SeqCst) >= 4);
}

#[test]
fn another_verifier_cannot_issue_or_consume_fixture_acceptance_stages() {
    let other = RegistrationVerifier::new("other.test", "https://other.test").unwrap();
    let (_, ready) = ready(false);
    let expected = expectations(ready.commitment(), false);
    assert!(other
        .begin_fixture_acceptances_using(ready, expected, proposal_tests::confirmation_runtime())
        .is_err());
    let (_, request, pending) = pending(false);
    let (organization, principal) = signatures(&request, false);
    assert!(other
        .finish_fixture_acceptances_using(
            pending,
            &organization,
            &principal,
            proposal_tests::confirmation_runtime()
        )
        .is_err());
    let (_, verified) = verified();
    assert!(other
        .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
        .is_err());
    let (verifier, verified) = self::verified();
    let (request, pending) = verifier
        .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
        .unwrap();
    let (device, assertion) = proposal_tests::evidence(&request);
    assert!(other
        .finish_fixture_proposal_using(
            pending,
            &device,
            &assertion,
            proposal_tests::confirmation_runtime()
        )
        .is_err());
}

#[test]
fn pending_fixture_proposal_requires_original_evidence_and_valid_final_possession() {
    for fault in 0..4 {
        let (verifier, verified) = verified();
        let (request, mut pending) = verifier
            .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
            .unwrap();
        let (mut device, mut assertion) = proposal_tests::evidence(&request);
        match fault {
            0 => pending.evidence.principal_signature[0] ^= 1,
            1 => pending.pending.packet.push(0),
            2 => device[0] ^= 1,
            3 => assertion.clear(),
            _ => unreachable!(),
        }
        assert!(verifier
            .finish_fixture_proposal_using(
                pending,
                &device,
                &assertion,
                proposal_tests::confirmation_runtime()
            )
            .is_err());
    }
}

#[test]
fn independent_vector_matches_version_two_and_all_four_signed_evidences() {
    let vector: Json = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/data/initial-acceptance-vector.json"
    )))
    .unwrap();
    let draft_json = vector["draft_json"].as_str().unwrap().as_bytes();
    let draft = Draft::parse_using(draft_json, proposal_tests::registration_runtime()).unwrap();
    assert_eq!(draft.canonical(), draft_json);
    assert_eq!(hex(&draft.commitment()), vector["draft_commitment"]);
    let verifier = RegistrationVerifier::new("vivac.test", "https://vivac.test").unwrap();
    let (request, pending) = verifier
        .start_using(
            draft.proof(),
            draft.commitment(),
            proposal_tests::registration_runtime(),
        )
        .unwrap();
    assert_eq!(
        hex(&decode(&request.options.challenge, 32).unwrap()),
        vector["registration_challenge"]
    );
    let registration = verifier
        .finish_using(
            pending,
            draft.proof(),
            draft.commitment(),
            vector["registration_json"].as_str().unwrap().as_bytes(),
            proposal_tests::registration_runtime(),
        )
        .unwrap();
    assert_eq!(
        hex(&registration.commitment()),
        vector["registration_commitment"]
    );
    let ready = verifier
        .prepare_proposal_using(draft, registration, proposal_tests::confirmation_runtime())
        .unwrap();
    assert_eq!(
        ready.packet(),
        vector["proposal_json"].as_str().unwrap().as_bytes()
    );
    assert_eq!(hex(&ready.commitment()), vector["proposal_commitment"]);
    let fields = &vector["fixture_expectations"];
    let expected = FixtureExpectations::new_fixture(
        fields["organization_id"].as_str().unwrap(),
        fields["operation_id"].as_str().unwrap(),
        crate::enrollment_proof::decode_hex::<32>(fields["proposal_commitment"].as_str().unwrap())
            .unwrap(),
        FixtureSigner::new(
            fields["organization_authority_id"].as_str().unwrap(),
            crate::enrollment_proof::decode_hex::<32>(
                fields["organization_authority_public_key"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap(),
        FixtureSigner::new(
            fields["principal_id"].as_str().unwrap(),
            crate::enrollment_proof::decode_hex::<32>(
                fields["principal_public_key"].as_str().unwrap(),
            )
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let (request, pending) = verifier
        .begin_fixture_acceptances_using(ready, expected, proposal_tests::confirmation_runtime())
        .unwrap();
    assert_eq!(
        hex(&decode(&request.organization_message, 512).unwrap()),
        vector["organization_message_hex"]
    );
    assert_eq!(
        hex(&decode(&request.principal_message, 512).unwrap()),
        vector["principal_message_hex"]
    );
    let organization = crate::enrollment_proof::decode_hex::<64>(
        vector["organization_signature_hex"].as_str().unwrap(),
    )
    .unwrap();
    let principal = crate::enrollment_proof::decode_hex::<64>(
        vector["principal_signature_hex"].as_str().unwrap(),
    )
    .unwrap();
    let verified = verifier
        .finish_fixture_acceptances_using(
            pending,
            &organization,
            &principal,
            proposal_tests::confirmation_runtime(),
        )
        .unwrap();
    let (request, pending) = verifier
        .start_fixture_proposal_using(verified, proposal_tests::confirmation_runtime())
        .unwrap();
    assert_eq!(
        hex(&decode(&request.options.challenge, 32).unwrap()),
        vector["challenge_hex"]
    );
    assert_eq!(
        hex(&decode(&request.signing_message, 128).unwrap()),
        vector["signing_message_hex"]
    );
    let device =
        crate::enrollment_proof::decode_hex::<64>(vector["final_signature_hex"].as_str().unwrap())
            .unwrap();
    let complete = verifier
        .finish_fixture_proposal_using(
            pending,
            &device,
            vector["assertion_json"].as_str().unwrap().as_bytes(),
            proposal_tests::confirmation_runtime(),
        )
        .unwrap();
    assert_eq!(
        complete.packet(),
        vector["proposal_json"].as_str().unwrap().as_bytes()
    );
    assert_eq!(complete.possession().counter(), 8);
    assert_eq!(
        complete.acceptances().organization_signature(),
        organization
    );
    assert_eq!(complete.acceptances().principal_signature(), principal);
}
