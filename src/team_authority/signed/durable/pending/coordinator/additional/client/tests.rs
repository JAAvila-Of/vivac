use super::*;
use crate::approval::tests::{fixture as assertion_fixture, fixture_binding_with_identity};
use crate::approval::{commitment, EnrollmentAssertionVerifier};
use ed25519_dalek::{Signer, SigningKey};

fn fixture() -> (
    FixtureClientExpectation,
    Proof,
    AssertionRequest,
    ExistingClientEndorsement,
    CandidateSelection,
) {
    let (verifier, _, _, proof) = assertion_fixture();
    let previous = SigningKey::from_bytes(&[0x62; 32]);
    let expected = FixtureClientExpectation {
        candidate: proof.challenge.clone(),
        organization: OrganizationId::parse(&proof.challenge.organization_id).unwrap(),
        principal: PrincipalId::parse(&proof.challenge.principal_id).unwrap(),
        credential: vec![0x21; 32],
        rp_id: "vivac.test".into(),
        origin: "https://vivac.test".into(),
        previous_device: DeviceId::parse("00000000000000000000000008").unwrap(),
        previous_credential: CredentialId::parse("00000000000000000000000009").unwrap(),
        previous_public_key: previous.verifying_key().to_bytes(),
        checkpoint: Checkpoint {
            revision: proof.challenge.base_revision,
            digest: [0xcd; 32],
        },
        policy: [0xef; 32],
    };
    let message = existing_client_message(
        &expected.organization,
        &expected.principal,
        &expected.previous_device,
        &expected.previous_credential,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
    )
    .unwrap();
    let endorsement = ExistingClientEndorsement {
        device: expected.previous_device.clone(),
        credential: expected.previous_credential.clone(),
        signature: previous.sign(&message).to_bytes(),
    };
    let scope = additional_context_digest(
        &expected.organization,
        &expected.principal,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
        &endorsement,
    )
    .unwrap();
    let binding =
        fixture_binding_with_identity(expected.organization.clone(), expected.principal.clone());
    let (request, _) = verifier.start_additional(&proof, &binding, scope).unwrap();
    let selection = CandidateSelection::from_reference(
        &proof,
        &proof.challenge.candidate_reference().unwrap(),
        request.context.issued_at,
    )
    .unwrap();
    (expected, proof, request, endorsement, selection)
}

#[test]
fn checked_client_parameters_preserve_the_expected_candidate_and_expire() {
    let (expected, proof, request, endorsement, selection) = fixture();
    let now = request.context.issued_at;
    let checked = check_request(&expected, selection, &proof, request, &endorsement, now).unwrap();
    assert_eq!(
        checked.candidate().canonical_bytes().unwrap(),
        expected.candidate.canonical_bytes().unwrap()
    );
    let options = checked.into_options(now + 1).unwrap();
    assert!(options.timeout <= (expected.candidate.expires_at - now - 1) * 1000);
    assert_eq!(options.rp_id, expected.rp_id);
    assert_eq!(
        options.user_verification,
        UserVerificationRequirement::Required
    );
}

#[test]
fn each_candidate_field_is_compared_with_the_independent_expectation() {
    for field in 0..11 {
        let (mut expected, proof, request, endorsement, selection) = fixture();
        let now = request.context.issued_at;
        let candidate = &mut expected.candidate;
        match field {
            0 => candidate.version = 2,
            1 => candidate.organization_id = "00000000000000000000000011".into(),
            2 => candidate.principal_id = "00000000000000000000000012".into(),
            3 => candidate.device_id = "00000000000000000000000013".into(),
            4 => candidate.credential_id = "00000000000000000000000014".into(),
            5 => {
                candidate.public_key = SigningKey::from_bytes(&[0x63; 32])
                    .verifying_key()
                    .to_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            }
            6 => candidate.operation_id = "00000000000000000000000016".into(),
            7 => candidate.base_revision += 1,
            8 => candidate.nonce = "24".repeat(32),
            9 => candidate.issued_at += 1,
            10 => candidate.expires_at -= 1,
            _ => unreachable!(),
        }
        assert!(
            check_request(&expected, selection, &proof, request, &endorsement, now).is_err(),
            "field {field}"
        );
    }
}

#[test]
fn a_validly_signed_substituted_remote_candidate_is_refused() {
    let (expected, mut proof, request, endorsement, selection) = fixture();
    let now = request.context.issued_at;
    proof.challenge.device_id = "00000000000000000000000017".into();
    proof = Proof::signed(proof.challenge, &SigningKey::from_bytes(&[0x61; 32]), now).unwrap();
    assert!(proof.verify(now).is_ok());
    assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
}

#[test]
fn generic_assertion_requests_are_not_additional_device_requests() {
    let (expected, proof, mut request, endorsement, selection) = fixture();
    let now = request.context.issued_at;
    let generic = commitment(
        &expected.candidate,
        &expected.organization,
        &expected.principal,
        &expected.credential,
        &expected.rp_id,
        &expected.origin,
        &request.context,
    )
    .unwrap();
    request.options.challenge = Base64UrlUnpadded::encode_string(&generic);
    assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
}

#[test]
fn changed_local_authority_policy_or_previous_key_is_refused() {
    for field in 0..8 {
        let (mut expected, proof, request, endorsement, selection) = fixture();
        let now = request.context.issued_at;
        match field {
            0 => expected.checkpoint.digest[0] ^= 1,
            1 => expected.checkpoint.revision += 1,
            2 => expected.policy[0] ^= 1,
            3 => {
                expected.previous_public_key = SigningKey::from_bytes(&[0x63; 32])
                    .verifying_key()
                    .to_bytes()
            }
            4 => expected.previous_device = DeviceId::parse("00000000000000000000000018").unwrap(),
            5 => {
                expected.previous_credential =
                    CredentialId::parse("00000000000000000000000019").unwrap()
            }
            6 => {
                expected.organization = OrganizationId::parse("00000000000000000000000011").unwrap()
            }
            7 => expected.principal = PrincipalId::parse("00000000000000000000000012").unwrap(),
            _ => unreachable!(),
        }
        assert!(
            check_request(&expected, selection, &proof, request, &endorsement, now).is_err(),
            "field {field}"
        );
    }
}

#[test]
fn modified_server_parameters_cannot_weaken_the_local_profile() {
    for field in 0..11 {
        let (expected, proof, mut request, endorsement, selection) = fixture();
        let now = request.context.issued_at;
        match field {
            0 => request.options.challenge.push('='),
            1 => request.options.rp_id = "other.test".into(),
            2 => request.options.user_verification = UserVerificationRequirement::Preferred,
            3 => request.options.allow_credentials.clear(),
            4 => request.options.allow_credentials[0].id = "AAAA".into(),
            5 => request.options.allow_credentials[0].type_ = "password",
            6 => request.options.timeout = 0,
            7 => request.options.timeout = 300_001,
            8 => request.context.nonce[0] ^= 1,
            9 => request.context.issued_at = now + 1,
            10 => request.context.expires_at += 1,
            _ => unreachable!(),
        }
        assert!(
            check_request(&expected, selection, &proof, request, &endorsement, now).is_err(),
            "field {field}"
        );
    }
}

#[test]
fn invalid_local_relying_party_configuration_is_refused() {
    for origin in [
        "http://vivac.test",
        "https://other.test",
        "https://vivac.test/extra",
        "https://user@vivac.test",
        "https://vivac.test?x=1",
        "https://vivac.test#x",
        "https://VIVAC.test",
        "https://vivac.test/",
    ] {
        let (mut expected, proof, request, endorsement, selection) = fixture();
        let now = request.context.issued_at;
        expected.origin = origin.into();
        assert!(
            check_request(&expected, selection, &proof, request, &endorsement, now).is_err(),
            "{origin}"
        );
    }
}

#[test]
fn a_corrupted_prior_signature_is_refused_locally() {
    let (expected, proof, request, mut endorsement, selection) = fixture();
    let now = request.context.issued_at;
    endorsement.signature[0] ^= 1;
    assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
}

#[test]
fn extensions_hints_and_additional_credentials_refuse_before_authentication() {
    for field in 0..4 {
        let (expected, proof, mut request, endorsement, selection) = fixture();
        let now = request.context.issued_at;
        match field {
            0 => request.options.extensions = Some(passki::AuthenticationExtensions::default()),
            1 => request
                .options
                .hints
                .push(passki::PublicKeyCredentialHint::SecurityKey),
            2 => request.options.allow_credentials[0]
                .transports
                .push(passki::AuthenticatorTransport::Usb),
            3 => {
                let (_, _, mut other, _, _) = fixture();
                request
                    .options
                    .allow_credentials
                    .push(other.options.allow_credentials.pop().unwrap());
            }
            _ => unreachable!(),
        }
        assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
    }
}

#[test]
fn future_issuance_refuses_even_with_a_matching_recomputed_digest() {
    let (expected, proof, mut request, endorsement, selection) = fixture();
    let now = request.context.issued_at;
    request.context.issued_at = now + 1;
    let context = additional_context_digest(
        &expected.organization,
        &expected.principal,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
        &endorsement,
    )
    .unwrap();
    let digest = additional_commitment(
        &expected.candidate,
        &expected.organization,
        &expected.principal,
        &expected.credential,
        &expected.rp_id,
        &expected.origin,
        &request.context,
        context,
    )
    .unwrap();
    request.options.challenge = Base64UrlUnpadded::encode_string(&digest);
    assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
}

#[test]
fn local_credential_and_configuration_bounds_are_enforced() {
    for field in 0..4 {
        let (mut expected, proof, request, endorsement, selection) = fixture();
        let now = request.context.issued_at;
        match field {
            0 => expected.credential.clear(),
            1 => expected.credential = vec![0; 1025],
            2 => expected.rp_id = "x".repeat(254),
            3 => expected.origin = "x".repeat(2049),
            _ => unreachable!(),
        }
        assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
    }
}

#[test]
fn expiry_and_clock_rollback_refuse_checked_parameters_at_use() {
    for expired in [false, true] {
        let (expected, proof, request, endorsement, selection) = fixture();
        let now = request.context.issued_at;
        let checked =
            check_request(&expected, selection, &proof, request, &endorsement, now).unwrap();
        assert!(checked
            .into_options(if expired {
                expected.candidate.expires_at
            } else {
                now - 1
            })
            .is_err());
    }
}

#[test]
fn a_recomputed_remote_digest_cannot_replace_the_local_configuration() {
    let (expected, proof, _, endorsement, selection) = fixture();
    let context = additional_context_digest(
        &expected.organization,
        &expected.principal,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
        &endorsement,
    )
    .unwrap();
    let remote = EnrollmentAssertionVerifier::new("other.test", "https://other.test").unwrap();
    let binding =
        fixture_binding_with_identity(expected.organization.clone(), expected.principal.clone());
    let (request, _) = remote.start_additional(&proof, &binding, context).unwrap();
    let now = request.context.issued_at;
    assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
}

#[test]
fn a_valid_remote_replacement_cannot_replace_the_selected_candidate() {
    let (mut expected, mut proof, _, mut endorsement, selection) = fixture();
    let now = proof.challenge.issued_at;
    proof.challenge.device_id = "00000000000000000000000017".into();
    proof = Proof::signed(proof.challenge, &SigningKey::from_bytes(&[0x61; 32]), now).unwrap();
    assert!(proof.verify(now).is_ok());
    expected.candidate = proof.challenge.clone();
    let previous = SigningKey::from_bytes(&[0x62; 32]);
    let message = existing_client_message(
        &expected.organization,
        &expected.principal,
        &expected.previous_device,
        &expected.previous_credential,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
    )
    .unwrap();
    endorsement.signature = previous.sign(&message).to_bytes();
    let context = additional_context_digest(
        &expected.organization,
        &expected.principal,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
        &endorsement,
    )
    .unwrap();
    let binding =
        fixture_binding_with_identity(expected.organization.clone(), expected.principal.clone());
    let verifier = EnrollmentAssertionVerifier::new(&expected.rp_id, &expected.origin).unwrap();
    let (request, _) = verifier
        .start_additional(&proof, &binding, context)
        .unwrap();
    let now = request.context.issued_at;
    assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
}

#[test]
fn selection_refuses_noncanonical_or_mismatched_references() {
    let (_, proof, request, _, _) = fixture();
    let now = request.context.issued_at;
    for reference in [
        String::new(),
        "00".repeat(31),
        "00".repeat(33),
        "AB".repeat(32),
        "gg".repeat(32),
        "00".repeat(32),
    ] {
        assert!(CandidateSelection::from_reference(&proof, &reference, now).is_err());
    }
}

#[test]
fn selection_refuses_corrupted_proofs_and_expired_candidates() {
    let (_, proof, request, _, _) = fixture();
    let now = request.context.issued_at;
    let reference = proof.challenge.candidate_reference().unwrap();
    let mut corrupted = proof.clone();
    let replacement = if &corrupted.signature[..2] == "00" {
        "01"
    } else {
        "00"
    };
    corrupted.signature.replace_range(..2, replacement);
    assert!(CandidateSelection::from_reference(&corrupted, &reference, now).is_err());
    assert!(
        CandidateSelection::from_reference(&proof, &reference, proof.challenge.expires_at,)
            .is_err()
    );
}

#[test]
fn selection_rejects_clock_rollback_while_the_proof_and_request_are_valid() {
    let (expected, proof, request, endorsement, _) = fixture();
    let now = request.context.issued_at;
    let selection = CandidateSelection::from_reference(
        &proof,
        &proof.challenge.candidate_reference().unwrap(),
        now + 1,
    )
    .unwrap();
    assert!(proof.verify(now).is_ok());
    assert!(now < request.context.expires_at);
    assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
}

#[test]
fn selection_does_not_extend_the_candidate_lifetime() {
    let (expected, proof, request, endorsement, selection) = fixture();
    let expires_at = proof.challenge.expires_at;
    assert!(check_request(
        &expected,
        selection,
        &proof,
        request,
        &endorsement,
        expires_at
    )
    .is_err());
}

#[test]
fn a_proof_changed_after_selection_is_refused() {
    let (expected, mut proof, request, endorsement, selection) = fixture();
    let now = request.context.issued_at;
    proof.challenge.nonce = "24".repeat(32);
    assert!(check_request(&expected, selection, &proof, request, &endorsement, now).is_err());
}

#[test]
fn explicit_selection_can_be_recreated_while_valid_without_durable_consumption() {
    let (expected, proof, request, endorsement, selection) = fixture();
    let now = request.context.issued_at;
    let challenge = request.options.challenge.clone();
    let checked = check_request(&expected, selection, &proof, request, &endorsement, now).unwrap();
    assert_eq!(
        checked.candidate().canonical_bytes().unwrap(),
        proof.challenge.canonical_bytes().unwrap()
    );
    assert_eq!(checked.into_options(now).unwrap().challenge, challenge);

    let selection = CandidateSelection::from_reference(
        &proof,
        &proof.challenge.candidate_reference().unwrap(),
        now,
    )
    .unwrap();
    let context = additional_context_digest(
        &expected.organization,
        &expected.principal,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
        &endorsement,
    )
    .unwrap();
    let binding =
        fixture_binding_with_identity(expected.organization.clone(), expected.principal.clone());
    let verifier = EnrollmentAssertionVerifier::new(&expected.rp_id, &expected.origin).unwrap();
    let (request, _) = verifier
        .start_additional(&proof, &binding, context)
        .unwrap();
    let now = request.context.issued_at;
    assert!(
        check_request(&expected, selection, &proof, request, &endorsement, now)
            .unwrap()
            .into_options(now)
            .is_ok()
    );
}
