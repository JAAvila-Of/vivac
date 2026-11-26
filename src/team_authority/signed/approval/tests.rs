use super::*;
use crate::enrollment_proof::Challenge;
use ciborium::Value;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

const ORIGIN: &str = "https://vivac.test";

#[test]
fn additional_assertions_reject_changed_context_and_crossed_ceremony_domains() {
    for case in 0..3 {
        let (verifier, mut binding, key, proof) = fixture();
        let digest = [0x37; 32];
        let (request, pending) = if case == 0 {
            verifier.start(&proof, &binding).unwrap()
        } else {
            verifier.start_additional(&proof, &binding, digest).unwrap()
        };
        let assertion = response(&request, &binding, &key);
        let result = if case == 1 {
            verifier.prepare(pending, &proof, &mut binding, &assertion)
        } else {
            let expected = if case == 2 { [0x38; 32] } else { digest };
            verifier.prepare_additional(pending, &proof, &mut binding, &assertion, expected)
        };
        assert!(matches!(result, Err(AssertionError::Refused)));
        drop(result);
        assert_eq!(fixture_state(&binding), (0, 0, false));
    }
}

#[test]
fn an_additional_assertion_with_the_same_context_can_publish_its_binding_update() {
    let (verifier, mut binding, key, proof) = fixture();
    let digest = [0x37; 32];
    let (request, pending) = verifier.start_additional(&proof, &binding, digest).unwrap();
    let expected = additional_commitment(
        &proof.challenge,
        &binding.organization,
        &binding.principal,
        &binding.passkey.credential_id,
        "vivac.test",
        ORIGIN,
        &request.context,
        digest,
    )
    .unwrap();
    assert_eq!(decode(&request.options.challenge, 32).unwrap(), expected);
    let assertion = response(&request, &binding, &key);
    verifier
        .prepare_additional(pending, &proof, &mut binding, &assertion, digest)
        .unwrap()
        .publish();
    assert_eq!(fixture_state(&binding), (1, 1, false));
}

pub(crate) fn fixture() -> (
    EnrollmentAssertionVerifier,
    TrustedPasskeyBinding,
    SigningKey,
    Proof,
) {
    fixture_at(crate::enrollment_proof::current_time().unwrap())
}

fn fixture_at(
    now: u64,
) -> (
    EnrollmentAssertionVerifier,
    TrustedPasskeyBinding,
    SigningKey,
    Proof,
) {
    assert!(
        supported_cpu(),
        "The declared Linux CPU profile is required"
    );
    let verifier = EnrollmentAssertionVerifier::new("vivac.test", ORIGIN).unwrap();
    let key = SigningKey::from_bytes(&[0x61; 32]);
    let mut public_key = Vec::new();
    ciborium::into_writer(
        &Value::Map(vec![
            (Value::Integer(1.into()), Value::Integer(1.into())),
            (Value::Integer(3.into()), Value::Integer((-8).into())),
            (Value::Integer((-1).into()), Value::Integer(6.into())),
            (
                Value::Integer((-2).into()),
                Value::Bytes(key.verifying_key().to_bytes().to_vec()),
            ),
        ]),
        &mut public_key,
    )
    .unwrap();
    let mut passkey = StoredPasskey::default();
    passkey.credential_id = vec![0x21; 32];
    passkey.public_key = public_key;
    passkey.algorithm = -8;
    let binding = TrustedPasskeyBinding::new(
        OrganizationId::parse("00000000000000000000000001").unwrap(),
        PrincipalId::parse("00000000000000000000000002").unwrap(),
        passkey,
    )
    .unwrap();
    let proof = Proof::signed(
        Challenge {
            version: 1,
            organization_id: "00000000000000000000000001".into(),
            principal_id: "00000000000000000000000003".into(),
            device_id: "00000000000000000000000004".into(),
            credential_id: "00000000000000000000000005".into(),
            public_key: key
                .verifying_key()
                .to_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            operation_id: "00000000000000000000000006".into(),
            base_revision: 7,
            nonce: "23".repeat(32),
            issued_at: now,
            expires_at: now + 300,
        },
        &key,
        now,
    )
    .unwrap();
    (verifier, binding, key, proof)
}

pub(crate) fn response(
    challenge: &AssertionRequest,
    binding: &TrustedPasskeyBinding,
    key: &SigningKey,
) -> Vec<u8> {
    signed_response(
        &client_data(challenge, ORIGIN),
        &authenticator_data("vivac.test", 0x05, 1),
        binding,
        |signed| key.sign(signed).to_bytes().to_vec(),
    )
}

pub(crate) fn fixture_state(binding: &TrustedPasskeyBinding) -> (u32, u64, bool) {
    (
        binding.passkey.counter,
        binding.generation,
        binding.passkey.bs,
    )
}

pub(crate) fn fixture_binding_with_identity(
    organization: OrganizationId,
    principal: PrincipalId,
) -> TrustedPasskeyBinding {
    let (_, binding, _, _) = fixture();
    TrustedPasskeyBinding::new(organization, principal, binding.passkey).unwrap()
}

#[test]
fn preparing_an_assertion_can_be_discarded_without_publishing_binding_updates() {
    let (verifier, mut binding, key, proof) = fixture();
    binding.passkey.be = true;
    let before = fixture_state(&binding);
    let (request, pending) = verifier.start(&proof, &binding).unwrap();
    let bytes = signed_response(
        &client_data(&request, ORIGIN),
        &authenticator_data("vivac.test", 0x1d, 1),
        &binding,
        |signed| key.sign(signed).to_bytes().to_vec(),
    );
    let prepared = verifier
        .prepare(pending, &proof, &mut binding, &bytes)
        .unwrap();
    drop(prepared);
    assert_eq!(fixture_state(&binding), before);
}

#[test]
fn publishing_a_prepared_assertion_updates_the_binding_and_returns_its_evidence() {
    let (verifier, mut binding, key, proof) = fixture();
    binding.passkey.be = true;
    let (request, pending) = verifier.start(&proof, &binding).unwrap();
    let bytes = signed_response(
        &client_data(&request, ORIGIN),
        &authenticator_data("vivac.test", 0x1d, 1),
        &binding,
        |signed| key.sign(signed).to_bytes().to_vec(),
    );
    let prepared = verifier
        .prepare(pending, &proof, &mut binding, &bytes)
        .unwrap();
    let evidence = prepared.publish();
    assert_eq!(fixture_state(&binding), (1, 1, true));
    assert!(binding.passkey.be);
    assert_eq!(evidence.counter, 1);
    assert_eq!(
        evidence.enrollment,
        proof.challenge.canonical_bytes().unwrap()
    );
    assert_eq!(evidence.organization, *binding.organization());
    assert_eq!(evidence.principal, *binding.principal());
    assert_eq!(evidence.credential, binding.passkey.credential_id);
}

#[test]
fn refusing_preparation_leaves_the_binding_unchanged() {
    let (verifier, mut binding, key, proof) = fixture();
    let before = fixture_state(&binding);
    let (request, pending) = verifier.start(&proof, &binding).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&response(&request, &binding, &key)).unwrap();
    value["response"]["signature"] = encode(&[0; 64]).into();
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        verifier
            .prepare(pending, &proof, &mut binding, &bytes)
            .err(),
        Some(AssertionError::Refused)
    );
    assert_eq!(fixture_state(&binding), before);
}

fn client_data(challenge: &AssertionRequest, origin: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "type": "webauthn.get", "challenge": challenge.options.challenge,
        "origin": origin, "crossOrigin": false,
    }))
    .unwrap()
}

fn authenticator_data(rp_id: &str, flags: u8, counter: u32) -> Vec<u8> {
    let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
    data.push(flags);
    data.extend_from_slice(&counter.to_be_bytes());
    data
}

fn signed_response(
    client: &[u8],
    data: &[u8],
    binding: &TrustedPasskeyBinding,
    sign: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Vec<u8> {
    let mut signed = data.to_vec();
    signed.extend_from_slice(&Sha256::digest(client));
    let signature = sign(&signed);
    serde_json::to_vec(&serde_json::json!({
        "id": Passki::base64_encode(&binding.passkey.credential_id),
        "rawId": Passki::base64_encode(&binding.passkey.credential_id),
        "type": "public-key",
        "response": {
            "clientDataJSON": Passki::base64_encode(client),
            "authenticatorData": Passki::base64_encode(data),
            "signature": Passki::base64_encode(&signature),
            "userHandle": Passki::base64_encode(&binding.expected_user_handle),
        },
        "clientExtensionResults": {},
    }))
    .unwrap()
}

fn valid_fixture_signature(bytes: &[u8], key: &SigningKey) {
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    let mut signed = decode(
        value["response"]["authenticatorData"].as_str().unwrap(),
        128,
    )
    .unwrap();
    let client = decode(
        value["response"]["clientDataJSON"].as_str().unwrap(),
        MAX_CLIENT_DATA_BYTES,
    )
    .unwrap();
    signed.extend_from_slice(&Sha256::digest(&client));
    let signature: [u8; 64] = decode(value["response"]["signature"].as_str().unwrap(), 64)
        .unwrap()
        .try_into()
        .unwrap();
    key.verifying_key()
        .verify_strict(&signed, &ed25519_dalek::Signature::from_bytes(&signature))
        .unwrap();
}

fn now() -> AssertionResult<u64> {
    Ok(1000)
}
fn earlier() -> AssertionResult<u64> {
    Ok(999)
}
fn expired() -> AssertionResult<u64> {
    Ok(1300)
}
fn unavailable_time() -> AssertionResult<u64> {
    Err(AssertionError::Unavailable)
}
fn unsupported_cpu() -> bool {
    false
}
fn unavailable_entropy(_: &mut [u8]) -> AssertionResult<()> {
    Err(AssertionError::Unavailable)
}
fn panic_ceremony(
    _: &Passki,
    _: &StoredPasskey,
) -> AssertionResult<(AuthenticationChallenge, AuthenticationState)> {
    panic!("Public entropy failure fixture")
}

fn start_at(
    verifier: &EnrollmentAssertionVerifier,
    proof: &Proof,
    binding: &TrustedPasskeyBinding,
) -> (AssertionRequest, PendingAssertion) {
    verifier
        .start_using(proof, binding, now, supported_cpu, system_ceremony)
        .unwrap()
}

fn finish_at(
    verifier: &EnrollmentAssertionVerifier,
    pending: PendingAssertion,
    proof: &Proof,
    binding: &mut TrustedPasskeyBinding,
    bytes: &[u8],
) -> AssertionResult<AssertionEvidence> {
    verifier.finish_using(pending, proof, binding, bytes, now, supported_cpu)
}

fn unchanged(binding: &TrustedPasskeyBinding, counter: u32, generation: u64, backup_state: bool) {
    assert_eq!(binding.passkey.counter, counter);
    assert_eq!(binding.generation, generation);
    assert_eq!(binding.passkey.bs, backup_state);
}

#[test]
fn virtual_assertion_binds_exact_enrollment_without_admission() {
    assert!(
        supported_cpu(),
        "The declared Linux CPU profile is required"
    );
    let (verifier, mut binding, key, proof) = fixture();
    let (challenge, pending) = verifier.start(&proof, &binding).unwrap();
    let bytes = response(&challenge, &binding, &key);
    let evidence = verifier
        .finish(pending, &proof, &mut binding, &bytes)
        .unwrap();
    assert_eq!(
        evidence.enrollment,
        proof.challenge.canonical_bytes().unwrap()
    );
    assert_eq!(evidence.organization, binding.organization);
    assert_eq!(evidence.principal, binding.principal);
    assert_eq!(evidence.credential, binding.passkey.credential_id);
    assert_eq!(evidence.counter, 1);
}

#[test]
fn signed_missing_presence_verification_and_unsupported_headers_refuse() {
    for flags in [0x00, 0x01, 0x04, 0x07, 0x0d, 0x15, 0x25, 0x45, 0x85] {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = signed_response(
            &client_data(&challenge, ORIGIN),
            &authenticator_data("vivac.test", flags, 1),
            &binding,
            |signed| key.sign(signed).to_bytes().to_vec(),
        );
        valid_fixture_signature(&bytes, &key);
        assert_eq!(
            finish_at(&verifier, pending, &proof, &mut binding, &bytes).err(),
            Some(AssertionError::Refused)
        );
        unchanged(&binding, 0, 0, false);
    }
    for length in [36, 38] {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let mut data = authenticator_data("vivac.test", 0x05, 1);
        data.resize(length, 0);
        let bytes = signed_response(
            &client_data(&challenge, ORIGIN),
            &data,
            &binding,
            |signed| key.sign(signed).to_bytes().to_vec(),
        );
        valid_fixture_signature(&bytes, &key);
        assert!(finish_at(&verifier, pending, &proof, &mut binding, &bytes).is_err());
        unchanged(&binding, 0, 0, false);
    }
}

#[test]
fn signed_wrong_origin_rp_challenge_and_ceremony_type_refuse() {
    for field in 0..5 {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let mut client: serde_json::Value =
            serde_json::from_slice(&client_data(&challenge, ORIGIN)).unwrap();
        let mut data = authenticator_data("vivac.test", 0x05, 1);
        match field {
            0 => client["origin"] = "https://other.test".into(),
            1 => data = authenticator_data("other.test", 0x05, 1),
            2 => client["challenge"] = encode(&[0x42; 32]).into(),
            3 => client["type"] = "webauthn.create".into(),
            _ => client["crossOrigin"] = true.into(),
        }
        let bytes = signed_response(
            &serde_json::to_vec(&client).unwrap(),
            &data,
            &binding,
            |signed| key.sign(signed).to_bytes().to_vec(),
        );
        valid_fixture_signature(&bytes, &key);
        assert!(finish_at(&verifier, pending, &proof, &mut binding, &bytes).is_err());
        unchanged(&binding, 0, 0, false);
    }
}

#[test]
fn exact_candidate_scope_and_possession_are_required_again_at_finish() {
    for field in 0..10 {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = response(&challenge, &binding, &key);
        let mut changed = proof.challenge.clone();
        let replacement = SigningKey::from_bytes(&[0x62; 32]);
        match field {
            0 => changed.organization_id = "00000000000000000000000011".into(),
            1 => changed.principal_id = "00000000000000000000000012".into(),
            2 => changed.device_id = "00000000000000000000000013".into(),
            3 => changed.credential_id = "00000000000000000000000014".into(),
            4 => changed.operation_id = "00000000000000000000000015".into(),
            5 => changed.base_revision += 1,
            6 => changed.nonce = "24".repeat(32),
            7 => {
                changed.issued_at -= 1;
                changed.expires_at -= 1;
            }
            8 => changed.expires_at -= 1,
            _ => {
                changed.public_key = replacement
                    .verifying_key()
                    .to_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            }
        }
        let changed =
            Proof::signed(changed, if field == 9 { &replacement } else { &key }, 1000).unwrap();
        assert!(finish_at(&verifier, pending, &changed, &mut binding, &bytes).is_err());
        unchanged(&binding, 0, 0, false);
    }
    for field in 0..3 {
        let (verifier, mut binding, key, mut proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = response(&challenge, &binding, &key);
        match field {
            0 => proof.status = "approved".into(),
            1 => proof.signature = "00".repeat(64),
            _ => proof.authority = "checked".into(),
        }
        assert!(finish_at(&verifier, pending, &proof, &mut binding, &bytes).is_err());
        unchanged(&binding, 0, 0, false);
    }
}

#[test]
fn credential_id_user_handle_type_and_signature_cannot_be_substituted() {
    for field in 0..8 {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let mut value: serde_json::Value =
            serde_json::from_slice(&response(&challenge, &binding, &key)).unwrap();
        match field {
            0 => value["id"] = encode(&[0x22; 32]).into(),
            1 => {
                value["id"] = encode(&[0x22; 32]).into();
                value["rawId"] = value["id"].clone();
            }
            2 => value["type"] = "other-key".into(),
            3 => value["response"]["userHandle"] = encode(b"00000000000000000000000009").into(),
            4 => value["response"]["signature"] = encode(&[0; 64]).into(),
            5 => value["clientExtensionResults"] = serde_json::json!({"prf":{"enabled":true}}),
            6 => value["authenticatorAttachment"] = "unknown".into(),
            _ => {
                value["response"]["signature"] =
                    encode(&crate::enrollment_proof::decode_hex::<64>(&proof.signature).unwrap())
                        .into()
            }
        }
        assert!(finish_at(
            &verifier,
            pending,
            &proof,
            &mut binding,
            &serde_json::to_vec(&value).unwrap()
        )
        .is_err());
        unchanged(&binding, 0, 0, false);
    }
}

#[test]
fn absent_user_handle_is_accepted_only_with_the_exact_single_credential() {
    let (verifier, mut binding, key, proof) = fixture_at(1000);
    let (challenge, pending) = start_at(&verifier, &proof, &binding);
    assert_eq!(
        pending.authentication.allowed_credentials,
        vec![binding.passkey.credential_id.clone()]
    );
    assert_eq!(
        challenge.options.user_verification,
        UserVerificationRequirement::Required
    );
    assert_eq!(
        pending.authentication.user_verification,
        UserVerificationRequirement::Required
    );
    let mut value: serde_json::Value =
        serde_json::from_slice(&response(&challenge, &binding, &key)).unwrap();
    value["response"]
        .as_object_mut()
        .unwrap()
        .remove("userHandle");
    finish_at(
        &verifier,
        pending,
        &proof,
        &mut binding,
        &serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    unchanged(&binding, 1, 1, false);
}

#[test]
fn duplicate_malformed_and_unknown_client_json_fields_refuse_signed_responses() {
    for field in 0..6 {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let client = String::from_utf8(client_data(&challenge, ORIGIN)).unwrap();
        let client = match field {
            0 => client.replacen('{', "{\"origin\":\"https://vivac.test\",", 1),
            1 => client.replacen("false", "\"false\"", 1),
            2 => client.replacen('{', "{\"topOrigin\":\"https://other.test\",", 1),
            3 => client.replacen('{', "{\"unknown\":true,", 1),
            4 => format!("{client}{{}}"),
            _ => "{".into(),
        };
        let bytes = signed_response(
            client.as_bytes(),
            &authenticator_data("vivac.test", 0x05, 1),
            &binding,
            |signed| key.sign(signed).to_bytes().to_vec(),
        );
        valid_fixture_signature(&bytes, &key);
        assert!(finish_at(&verifier, pending, &proof, &mut binding, &bytes).is_err());
        unchanged(&binding, 0, 0, false);
    }
}

#[test]
fn outer_duplicate_fields_input_bounds_and_noncanonical_base64_refuse() {
    for field in 0..9 {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = response(&challenge, &binding, &key);
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let bytes = match field {
            0 => String::from_utf8(bytes)
                .unwrap()
                .replacen('{', "{\"id\":\"duplicate\",", 1)
                .into_bytes(),
            1 => vec![b' '; MAX_ASSERTION_BYTES + 1],
            2 => Vec::new(),
            3 => {
                value["rawId"] = format!("{}=", value["rawId"].as_str().unwrap()).into();
                value["id"] = value["rawId"].clone();
                serde_json::to_vec(&value).unwrap()
            }
            4 => {
                value["response"]["signature"] =
                    format!("{}=", value["response"]["signature"].as_str().unwrap()).into();
                serde_json::to_vec(&value).unwrap()
            }
            5 => {
                value["response"]["clientDataJSON"] =
                    encode(&vec![b' '; MAX_CLIENT_DATA_BYTES + 1]).into();
                serde_json::to_vec(&value).unwrap()
            }
            6 => {
                value["clientExtensionResults"] = serde_json::Value::Null;
                serde_json::to_vec(&value).unwrap()
            }
            7 => {
                value["unknown"] = true.into();
                serde_json::to_vec(&value).unwrap()
            }
            _ => {
                value["response"]["authenticatorData"] = "*".into();
                serde_json::to_vec(&value).unwrap()
            }
        };
        assert!(finish_at(&verifier, pending, &proof, &mut binding, &bytes).is_err());
        unchanged(&binding, 0, 0, false);
    }
}

#[test]
fn counter_regression_and_old_concurrent_ceremonies_leave_current_binding_unchanged() {
    let (verifier, mut binding, key, proof) = fixture_at(1000);
    let (first, first_pending) = start_at(&verifier, &proof, &binding);
    let (second, second_pending) = start_at(&verifier, &proof, &binding);
    let bytes = response(&first, &binding, &key);
    finish_at(&verifier, first_pending, &proof, &mut binding, &bytes).unwrap();
    let bytes = signed_response(
        &client_data(&second, ORIGIN),
        &authenticator_data("vivac.test", 0x05, 2),
        &binding,
        |signed| key.sign(signed).to_bytes().to_vec(),
    );
    assert!(finish_at(&verifier, second_pending, &proof, &mut binding, &bytes).is_err());
    unchanged(&binding, 1, 1, false);
    for counter in [0, 1] {
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = signed_response(
            &client_data(&challenge, ORIGIN),
            &authenticator_data("vivac.test", 0x05, counter),
            &binding,
            |signed| key.sign(signed).to_bytes().to_vec(),
        );
        valid_fixture_signature(&bytes, &key);
        assert!(finish_at(&verifier, pending, &proof, &mut binding, &bytes).is_err());
        unchanged(&binding, 1, 1, false);
    }
}

#[test]
fn zero_counter_and_signed_backup_state_updates_are_not_confused_with_failure() {
    let (verifier, mut binding, key, proof) = fixture_at(1000);
    for generation in [1, 2] {
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = signed_response(
            &client_data(&challenge, ORIGIN),
            &authenticator_data("vivac.test", 0x05, 0),
            &binding,
            |signed| key.sign(signed).to_bytes().to_vec(),
        );
        finish_at(&verifier, pending, &proof, &mut binding, &bytes).unwrap();
        unchanged(&binding, 0, generation, false);
    }
    binding.passkey.be = true;
    for (flags, counter, backup_state) in [(0x1d, 1, true), (0x0d, 2, false)] {
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = signed_response(
            &client_data(&challenge, ORIGIN),
            &authenticator_data("vivac.test", flags, counter),
            &binding,
            |signed| key.sign(signed).to_bytes().to_vec(),
        );
        finish_at(&verifier, pending, &proof, &mut binding, &bytes).unwrap();
        assert_eq!(binding.passkey.counter, counter);
        assert_eq!(binding.passkey.bs, backup_state);
    }
}

#[test]
fn replacing_a_binding_or_verifier_configuration_invalidates_pending_ceremony() {
    for replacement in 0..3 {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = response(&challenge, &binding, &key);
        if replacement == 0 {
            binding = TrustedPasskeyBinding::new(
                binding.organization.clone(),
                binding.principal.clone(),
                binding.passkey.clone(),
            )
            .unwrap();
            assert!(finish_at(&verifier, pending, &proof, &mut binding, &bytes).is_err());
        } else {
            let other = if replacement == 1 {
                EnrollmentAssertionVerifier::new("vivac.test", "https://vivac.test:8443").unwrap()
            } else {
                EnrollmentAssertionVerifier::new("other.test", "https://other.test").unwrap()
            };
            assert!(finish_at(&other, pending, &proof, &mut binding, &bytes).is_err());
        }
        unchanged(&binding, 0, 0, false);
    }
}

#[test]
fn clock_cpu_entropy_and_generation_failures_never_publish_assertion_evidence() {
    let (verifier, mut binding, key, proof) = fixture_at(1000);
    assert_eq!(
        verifier
            .start_using(
                &proof,
                &binding,
                unavailable_time,
                supported_cpu,
                system_ceremony
            )
            .err(),
        Some(AssertionError::Unavailable)
    );
    assert_eq!(
        verifier
            .start_using(&proof, &binding, now, unsupported_cpu, system_ceremony)
            .err(),
        Some(AssertionError::Unavailable)
    );
    assert_eq!(
        verifier
            .start_using(&proof, &binding, now, supported_cpu, panic_ceremony)
            .err(),
        Some(AssertionError::Unavailable)
    );
    assert_eq!(
        TrustedPasskeyBinding::new_using(
            binding.organization.clone(),
            binding.principal.clone(),
            binding.passkey.clone(),
            unavailable_entropy
        )
        .err(),
        Some(AssertionError::Unavailable)
    );
    for clock in [unavailable_time as fn() -> AssertionResult<u64>, expired] {
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = response(&challenge, &binding, &key);
        assert!(verifier
            .finish_using(pending, &proof, &mut binding, &bytes, clock, supported_cpu)
            .is_err());
        unchanged(&binding, 0, 0, false);
    }
    let (challenge, pending) = start_at(&verifier, &proof, &binding);
    let bytes = response(&challenge, &binding, &key);
    assert_eq!(
        verifier
            .finish_using(pending, &proof, &mut binding, &bytes, now, unsupported_cpu)
            .err(),
        Some(AssertionError::Unavailable)
    );
    binding.generation = u64::MAX;
    let (challenge, pending) = start_at(&verifier, &proof, &binding);
    let bytes = response(&challenge, &binding, &key);
    assert_eq!(
        finish_at(&verifier, pending, &proof, &mut binding, &bytes).err(),
        Some(AssertionError::Refused)
    );
    unchanged(&binding, 0, u64::MAX, false);
}

#[test]
fn backwards_clock_before_ceremony_start_refuses_even_while_candidate_is_valid() {
    let (verifier, mut binding, key, proof) = fixture_at(900);
    let (challenge, pending) = start_at(&verifier, &proof, &binding);
    let bytes = response(&challenge, &binding, &key);
    proof.verify(999).unwrap();
    assert_eq!(
        verifier
            .finish_using(
                pending,
                &proof,
                &mut binding,
                &bytes,
                earlier,
                supported_cpu
            )
            .err(),
        Some(AssertionError::Refused)
    );
    unchanged(&binding, 0, 0, false);
}

#[test]
fn configuration_binding_limits_and_invalid_possession_fail_closed() {
    for (rp, origin) in [
        ("vivac.test", "http://vivac.test"),
        ("vivac.test", "https://sub.vivac.test"),
        ("vivac.test", "https://user@vivac.test"),
        ("vivac.test", "https://@vivac.test"),
        ("vivac.test", "https://vivac.test/path"),
        ("vivac.test", "https://vivac.test?query"),
        ("vivac.test", "https://vivac.test#fragment"),
    ] {
        assert!(EnrollmentAssertionVerifier::new(rp, origin).is_err());
    }
    let (verifier, binding, _, mut proof) = fixture_at(1000);
    for field in 0..6 {
        let mut passkey = binding.passkey.clone();
        match field {
            0 => passkey.credential_id.clear(),
            1 => passkey.credential_id = vec![0; MAX_CREDENTIAL_BYTES + 1],
            2 => passkey.public_key.clear(),
            3 => passkey.public_key = vec![0; MAX_PUBLIC_KEY_BYTES + 1],
            4 => passkey.algorithm = -257,
            _ => passkey.bs = true,
        }
        assert_eq!(
            TrustedPasskeyBinding::new(
                binding.organization.clone(),
                binding.principal.clone(),
                passkey
            )
            .err(),
            Some(AssertionError::Refused)
        );
    }
    proof.status = "approved".into();
    assert_eq!(
        verifier
            .start_using(&proof, &binding, now, supported_cpu, system_ceremony)
            .err(),
        Some(AssertionError::Refused)
    );
}

#[test]
fn wrong_or_malformed_stored_public_key_does_not_accept_an_otherwise_valid_assertion() {
    for malformed in [false, true] {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let bytes = response(&challenge, &binding, &key);
        valid_fixture_signature(&bytes, &key);
        if malformed {
            binding.passkey.public_key = vec![0];
        } else {
            let replacement = SigningKey::from_bytes(&[0x63; 32]);
            binding.passkey.public_key.clear();
            ciborium::into_writer(
                &Value::Map(vec![
                    (Value::Integer(1.into()), Value::Integer(1.into())),
                    (Value::Integer(3.into()), Value::Integer((-8).into())),
                    (Value::Integer((-1).into()), Value::Integer(6.into())),
                    (
                        Value::Integer((-2).into()),
                        Value::Bytes(replacement.verifying_key().to_bytes().to_vec()),
                    ),
                ]),
                &mut binding.passkey.public_key,
            )
            .unwrap();
        }
        assert!(finish_at(&verifier, pending, &proof, &mut binding, &bytes).is_err());
        unchanged(&binding, 0, 0, false);
    }
}

#[test]
fn es256_public_rfc_fixture_verifies_and_rejects_signed_missing_uv_or_changed_challenge() {
    assert!(
        supported_cpu(),
        "The declared Linux CPU profile is required"
    );
    use graviola::signing::ecdsa::{SigningKey as P256SigningKey, P256};
    // Public test key and coordinates from RFC 6979, Appendix A.2.5.
    let scalar = crate::enrollment_proof::decode_hex::<32>(
        "c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721",
    )
    .unwrap();
    let x = crate::enrollment_proof::decode_hex::<32>(
        "60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6",
    )
    .unwrap();
    let y = crate::enrollment_proof::decode_hex::<32>(
        "7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299",
    )
    .unwrap();
    let mut der = vec![0x30, 0x31, 0x02, 0x01, 0x01, 0x04, 0x20];
    der.extend_from_slice(&scalar);
    der.extend_from_slice(&[
        0xa0, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07,
    ]);
    let signing = P256SigningKey::<P256>::from_sec1_der(&der).unwrap();
    let (verifier, mut binding, _, proof) = fixture_at(1000);
    binding.passkey.algorithm = -7;
    binding.passkey.public_key.clear();
    ciborium::into_writer(
        &Value::Map(vec![
            (Value::Integer(1.into()), Value::Integer(2.into())),
            (Value::Integer(3.into()), Value::Integer((-7).into())),
            (Value::Integer((-1).into()), Value::Integer(1.into())),
            (Value::Integer((-2).into()), Value::Bytes(x.to_vec())),
            (Value::Integer((-3).into()), Value::Bytes(y.to_vec())),
        ]),
        &mut binding.passkey.public_key,
    )
    .unwrap();
    for case in 0..3 {
        let (challenge, pending) = start_at(&verifier, &proof, &binding);
        let mut client: serde_json::Value =
            serde_json::from_slice(&client_data(&challenge, ORIGIN)).unwrap();
        if case == 2 {
            client["challenge"] = encode(&[0x42; 32]).into();
        }
        let bytes = signed_response(
            &serde_json::to_vec(&client).unwrap(),
            &authenticator_data(
                "vivac.test",
                if case == 1 { 0x01 } else { 0x05 },
                if case == 0 { 1 } else { 2 },
            ),
            &binding,
            |signed| {
                let mut signature = [0; 80];
                signing
                    .sign_asn1::<graviola::hashing::Sha256>(&[signed], &mut signature)
                    .unwrap()
                    .to_vec()
            },
        );
        let result = finish_at(&verifier, pending, &proof, &mut binding, &bytes);
        if case == 0 {
            assert_eq!(result.unwrap().counter, 1);
        } else {
            assert_eq!(result.err(), Some(AssertionError::Refused));
        }
        unchanged(&binding, 1, 1, false);
    }
}

fn golden_candidate() -> Challenge {
    Challenge {
        version: 1,
        organization_id: "00000000000000000000000001".into(),
        principal_id: "00000000000000000000000003".into(),
        device_id: "00000000000000000000000004".into(),
        credential_id: "00000000000000000000000005".into(),
        public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
        operation_id: "00000000000000000000000006".into(),
        base_revision: 7,
        nonce: "23".repeat(32),
        issued_at: 1000,
        expires_at: 1300,
    }
}

#[test]
fn commitment_matches_independent_fixed_wire_vector() {
    let candidate = golden_candidate();
    let organization = OrganizationId::parse("00000000000000000000000001").unwrap();
    let principal = PrincipalId::parse("00000000000000000000000002").unwrap();
    let context = AssertionContext {
        nonce: [0xab; 32],
        issued_at: 1100,
        expires_at: 1300,
    };
    let digest = commitment(
        &candidate,
        &organization,
        &principal,
        b"fixture-credential",
        "vivac.test",
        ORIGIN,
        &context,
    )
    .unwrap();
    // Independently encoded with Python's standard-library SHA-256.
    assert_eq!(
        digest,
        crate::enrollment_proof::decode_hex::<32>(
            "7fa911c164c31ad1ff9a6a84f52ca8dcd24492a24ad68a120691b7d08ce70fd3"
        )
        .unwrap()
    );
    for field in 0..3 {
        let mut changed = context;
        match field {
            0 => changed.issued_at = 999,
            1 => changed.issued_at = 1300,
            _ => changed.expires_at = 1301,
        }
        assert_eq!(
            commitment(
                &candidate,
                &organization,
                &principal,
                b"fixture-credential",
                "vivac.test",
                ORIGIN,
                &changed
            )
            .err(),
            Some(AssertionError::Refused)
        );
    }
    for (credential, rp, origin) in [
        (Vec::new(), "vivac.test".to_string(), ORIGIN.to_string()),
        (
            vec![0; MAX_CREDENTIAL_BYTES + 1],
            "vivac.test".to_string(),
            ORIGIN.to_string(),
        ),
        (
            b"fixture-credential".to_vec(),
            "x".repeat(254),
            ORIGIN.to_string(),
        ),
        (
            b"fixture-credential".to_vec(),
            "vivac.test".to_string(),
            "x".repeat(2049),
        ),
    ] {
        assert!(commitment(
            &candidate,
            &organization,
            &principal,
            &credential,
            &rp,
            &origin,
            &context
        )
        .is_err());
    }
}

#[test]
fn public_ceremony_context_can_be_checked_against_the_locally_expected_candidate() {
    let (verifier, binding, key, proof) = fixture_at(1000);
    let (request, _pending) = start_at(&verifier, &proof, &binding);
    let expected = commitment(
        &proof.challenge,
        &binding.organization,
        &binding.principal,
        &binding.passkey.credential_id,
        "vivac.test",
        ORIGIN,
        &request.context,
    )
    .unwrap();
    assert_eq!(decode(&request.options.challenge, 32).unwrap(), expected);
    let mut changed = proof.challenge.clone();
    changed.operation_id = "00000000000000000000000009".into();
    let changed = Proof::signed(changed, &key, 1000).unwrap();
    let other = commitment(
        &changed.challenge,
        &binding.organization,
        &binding.principal,
        &binding.passkey.credential_id,
        "vivac.test",
        ORIGIN,
        &request.context,
    )
    .unwrap();
    assert_ne!(expected, other);
    let mut changed_context = request.context;
    changed_context.nonce[0] ^= 1;
    assert_ne!(
        expected,
        commitment(
            &proof.challenge,
            &binding.organization,
            &binding.principal,
            &binding.passkey.credential_id,
            "vivac.test",
            ORIGIN,
            &changed_context
        )
        .unwrap()
    );
}

#[test]
fn changing_server_association_or_context_cannot_rebind_a_captured_assertion() {
    for field in 0..3 {
        let (verifier, mut binding, key, proof) = fixture_at(1000);
        let (request, mut pending) = start_at(&verifier, &proof, &binding);
        let bytes = response(&request, &binding, &key);
        valid_fixture_signature(&bytes, &key);
        let mut changed = proof.clone();
        match field {
            0 => {
                let mut challenge = proof.challenge.clone();
                challenge.operation_id = "00000000000000000000000009".into();
                changed = Proof::signed(challenge, &key, 1000).unwrap();
                pending.enrollment = changed.challenge.canonical_bytes().unwrap();
            }
            1 => pending.context.nonce[0] ^= 1,
            _ => pending.context.issued_at = 999,
        }
        assert!(finish_at(&verifier, pending, &changed, &mut binding, &bytes).is_err());
        unchanged(&binding, 0, 0, false);
    }
}
