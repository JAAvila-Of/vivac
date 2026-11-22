use super::*;
use ed25519_dalek::Signer;
use serde_json::{json, Value as Json};

const RP: &str = "vivac.test";
const ORIGIN: &str = "https://vivac.test";
const DRAFT: [u8; 32] = [0x52; 32];

fn signing() -> ed25519_dalek::SigningKey {
    // RFC 8032, section 7.1, test 1; public test material only.
    ed25519_dalek::SigningKey::from_bytes(&[
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ])
}

fn fixture(counter: u32) -> (RegistrationVerifier, Proof, UnconfirmedRegistration) {
    let now = system_time().unwrap();
    let key = signing();
    let proof = Proof::signed(
        crate::enrollment_proof::Challenge {
            version: 1,
            organization_id: "00000000000000000000000001".into(),
            principal_id: "00000000000000000000000003".into(),
            device_id: "00000000000000000000000004".into(),
            credential_id: "00000000000000000000000005".into(),
            public_key: key
                .verifying_key()
                .to_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
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
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    let (request, pending) = verifier.start(&proof, DRAFT).unwrap();
    let integer = |n: i32| Value::Integer(n.into());
    let key = Value::Map(vec![
        (integer(1), integer(1)),
        (integer(3), integer(-8)),
        (integer(-1), integer(6)),
        (
            integer(-2),
            Value::Bytes(key.verifying_key().to_bytes().to_vec()),
        ),
    ]);
    let mut auth = Sha256::digest(RP.as_bytes()).to_vec();
    auth.push(0x5d);
    auth.extend_from_slice(&counter.to_be_bytes());
    auth.extend_from_slice(&[0x45; 16]);
    auth.extend_from_slice(&32_u16.to_be_bytes());
    auth.extend_from_slice(&[0x31; 32]);
    ciborium::into_writer(&key, &mut auth).unwrap();
    let mut attestation = Vec::new();
    ciborium::into_writer(
        &Value::Map(vec![
            (Value::Text("fmt".into()), Value::Text("none".into())),
            (Value::Text("authData".into()), Value::Bytes(auth)),
            (Value::Text("attStmt".into()), Value::Map(vec![])),
        ]),
        &mut attestation,
    )
    .unwrap();
    let response = serde_json::to_vec(&json!({
        "id":encode(&[0x31;32]), "rawId":encode(&[0x31;32]), "type":"public-key",
        "response":{"clientDataJSON":encode(&serde_json::to_vec(&json!({
            "type":"webauthn.create", "challenge":request.options.challenge,
            "origin":ORIGIN})).unwrap()), "attestationObject":encode(&attestation)}
    }))
    .unwrap();
    let registration = verifier.finish(pending, &proof, DRAFT, &response).unwrap();
    (verifier, proof, registration)
}

fn pending(
    counter: u32,
) -> (
    RegistrationVerifier,
    Proof,
    FinalAssertionRequest,
    PendingConfirmation,
) {
    let (verifier, proof, registration) = fixture(counter);
    let registration_commitment = registration.commitment();
    let (context, reserved) = verifier
        .reserve_confirmation(registration, &proof, DRAFT)
        .unwrap();
    let binding = ConfirmationBinding {
        proposal: [0x63; 32],
        registration: registration_commitment,
        context,
    };
    let (request, pending) = verifier
        .start_confirmation(reserved, &proof, DRAFT, binding)
        .unwrap();
    (verifier, proof, request, pending)
}

fn assertion(request: &FinalAssertionRequest, counter: u32, flags: u8) -> Json {
    let client = serde_json::to_vec(&json!({"type":"webauthn.get",
        "challenge":request.options.challenge,"origin":ORIGIN}))
    .unwrap();
    let mut auth = Sha256::digest(RP.as_bytes()).to_vec();
    auth.push(flags);
    auth.extend_from_slice(&counter.to_be_bytes());
    let mut message = auth.clone();
    message.extend_from_slice(&Sha256::digest(&client));
    json!({"id":encode(&[0x31;32]),"rawId":encode(&[0x31;32]),"type":"public-key",
        "response":{"clientDataJSON":encode(&client),"authenticatorData":encode(&auth),
        "signature":encode(&signing().sign(&message).to_bytes())},"clientExtensionResults":{}})
}

fn finish(
    value: Json,
    fixture: (
        RegistrationVerifier,
        Proof,
        FinalAssertionRequest,
        PendingConfirmation,
    ),
) -> RegistrationResult<PossessionConfirmation> {
    let (verifier, proof, request, pending) = fixture;
    let signature = signing()
        .sign(&decode(&request.signing_message, 128).unwrap())
        .to_bytes();
    verifier.finish_confirmation(
        pending,
        &proof,
        DRAFT,
        request.binding,
        &signature,
        &serde_json::to_vec(&value).unwrap(),
    )
}

#[test]
fn final_possession_preserves_original_registration() {
    let fixture = pending(9);
    let original = fixture.3.registration.response.clone();
    let value = assertion(&fixture.2, 10, 0x1d);
    let result = finish(value, fixture).unwrap();
    assert_eq!(result.registration().response, original);
    assert_eq!(result.registration().descriptor.counter, 9);
    assert_eq!(
        result.binding().registration,
        result.registration().commitment()
    );
    assert_eq!(result.counter(), 10);
    assert!(result.backup_state());
    assert!(!result.assertion().is_empty());
    assert_eq!(result.device_signature().len(), 64);
}

#[test]
fn exact_credential_request_requires_uv_and_binds_both_challenges() {
    let (_, _, request, pending) = pending(9);
    assert_eq!(request.options.allow_credentials.len(), 1);
    assert_eq!(request.options.allow_credentials[0].id, encode(&[0x31; 32]));
    assert_eq!(
        request.options.user_verification,
        UserVerificationRequirement::Required
    );
    assert_eq!(
        decode(&request.options.challenge, 32).unwrap(),
        pending.state.challenge
    );
    assert!(request.options.extensions.is_none());
}

#[test]
fn reservation_refuses_changed_provenance() {
    for case in 0..9 {
        let (verifier, mut proof, mut registration) = fixture(9);
        let mut draft = DRAFT;
        match case {
            0 => draft[0] ^= 1,
            1 => {
                let replacement = if proof.signature.starts_with('f') {
                    "0"
                } else {
                    "f"
                };
                proof.signature.replace_range(..1, replacement);
            }
            2 => registration.rp.push('x'),
            3 => registration.origin.push('x'),
            4 => registration.commitment[0] ^= 1,
            5 => registration.response.push(b' '),
            6 => registration.descriptor.counter += 1,
            7 => registration.stored.counter += 1,
            _ => registration.deadline = Instant::now(),
        }
        assert!(verifier
            .reserve_confirmation(registration, &proof, draft)
            .is_err());
    }
}

#[test]
fn start_refuses_substituted_context_and_registration() {
    for case in 0..4 {
        let (verifier, proof, registration) = fixture(9);
        let commitment = registration.commitment();
        let (context, reserved) = verifier
            .reserve_confirmation(registration, &proof, DRAFT)
            .unwrap();
        let mut binding = ConfirmationBinding {
            proposal: [1; 32],
            registration: commitment,
            context,
        };
        match case {
            0 => binding.registration[0] ^= 1,
            1 => binding.context.nonce[0] ^= 1,
            2 => binding.context.issued_at += 1,
            _ => binding.context.expires_at += 1,
        }
        assert!(verifier
            .start_confirmation(reserved, &proof, DRAFT, binding)
            .is_err());
    }
}

#[test]
fn finish_refuses_changed_proposal_or_context() {
    for case in 0..3 {
        let (verifier, proof, request, pending) = pending(9);
        let value = serde_json::to_vec(&assertion(&request, 10, 0x1d)).unwrap();
        let signature = signing()
            .sign(&decode(&request.signing_message, 128).unwrap())
            .to_bytes();
        let mut binding = request.binding;
        match case {
            0 => binding.proposal[0] ^= 1,
            1 => binding.registration[0] ^= 1,
            _ => binding.context.nonce[0] ^= 1,
        }
        assert!(verifier
            .finish_confirmation(pending, &proof, DRAFT, binding, &signature, &value)
            .is_err());
    }
}

#[test]
fn final_device_signature_refuses_wrong_key_message_and_length() {
    for case in 0..4 {
        let (verifier, proof, request, pending) = pending(9);
        let value = serde_json::to_vec(&assertion(&request, 10, 0x1d)).unwrap();
        let message = decode(&request.signing_message, 128).unwrap();
        let mut signature = match case {
            0 => ed25519_dalek::SigningKey::from_bytes(&[7; 32])
                .sign(&message)
                .to_bytes()
                .to_vec(),
            1 => signing().sign(b"unrelated message").to_bytes().to_vec(),
            _ => signing().sign(&message).to_bytes().to_vec(),
        };
        if case == 2 {
            signature.pop();
        }
        if case == 3 {
            signature.push(0);
        }
        assert!(verifier
            .finish_confirmation(pending, &proof, DRAFT, request.binding, &signature, &value)
            .is_err());
    }
}

#[test]
fn assertion_refuses_wrong_purpose_challenge_origin_and_client_fields() {
    for (field, replacement) in [
        ("type", json!("webauthn.create")),
        ("challenge", json!(encode(&[0; 32]))),
        ("origin", json!("https://other.test")),
        ("crossOrigin", json!(true)),
        ("topOrigin", json!(ORIGIN)),
        ("extra", json!(true)),
    ] {
        let fixture = pending(9);
        let mut value = assertion(&fixture.2, 10, 0x1d);
        let bytes = decode(
            value["response"]["clientDataJSON"].as_str().unwrap(),
            MAX_COMPONENT,
        )
        .unwrap();
        let mut client: Json = serde_json::from_slice(&bytes).unwrap();
        client[field] = replacement;
        value["response"]["clientDataJSON"] = json!(encode(&serde_json::to_vec(&client).unwrap()));
        assert!(finish(value, fixture).is_err());
    }
}

#[test]
fn assertion_refuses_missing_presence_verification_and_unsupported_flags() {
    for flags in [0x1c, 0x19, 0x15, 0x05, 0x1f, 0x3d, 0x5d, 0x9d] {
        let fixture = pending(9);
        let value = assertion(&fixture.2, 10, flags);
        assert!(finish(value, fixture).is_err());
    }
}

#[test]
fn counter_policy_refuses_unchanged_nonzero_and_rollback_but_accepts_both_zero() {
    for counter in [0, 8, 9] {
        let fixture = pending(9);
        let value = assertion(&fixture.2, counter, 0x1d);
        assert!(finish(value, fixture).is_err());
    }
    let fixture = pending(0);
    let value = assertion(&fixture.2, 0, 0x1d);
    assert_eq!(finish(value, fixture).unwrap().counter(), 0);
}

#[test]
fn assertion_refuses_wrong_credential_handle_signature_and_unknown_fields() {
    for case in 0..8 {
        let fixture = pending(9);
        let mut value = assertion(&fixture.2, 10, 0x1d);
        match case {
            0 => value["id"] = json!(encode(&[0; 32])),
            1 => value["rawId"] = json!(encode(&[0; 32])),
            2 => value["response"]["userHandle"] = json!(encode(b"other")),
            3 => value["response"]["signature"] = json!(encode(&[0; 64])),
            4 => value["response"]["extra"] = json!(true),
            5 => value["clientExtensionResults"] = json!({"prf":{}}),
            6 => value["authenticatorAttachment"] = json!("invalid"),
            _ => value["extra"] = json!(true),
        }
        assert!(finish(value, fixture).is_err());
    }
}

#[test]
fn exact_credential_allows_absent_null_and_matching_user_handle() {
    for handle in [Json::Null, json!(encode(b"00000000000000000000000003"))] {
        let fixture = pending(9);
        let mut value = assertion(&fixture.2, 10, 0x1d);
        value["response"]["userHandle"] = handle;
        assert!(finish(value, fixture).is_ok());
    }
}

#[test]
fn malformed_oversized_and_duplicate_json_refuse() {
    for raw in [
        b"{".to_vec(),
        vec![b' '; MAX_RESPONSE + 1],
        b"{\"id\":\"x\",\"id\":\"x\"}".to_vec(),
    ] {
        let (verifier, proof, request, pending) = pending(9);
        let signature = signing()
            .sign(&decode(&request.signing_message, 128).unwrap())
            .to_bytes();
        assert!(verifier
            .finish_confirmation(pending, &proof, DRAFT, request.binding, &signature, &raw)
            .is_err());
    }
}

thread_local! {
    static CLOCK: std::cell::Cell<(u64, u64, usize)> = const { std::cell::Cell::new((0, 0, 0)) };
    static FACTORY_CASE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn controlled_clock() -> RegistrationResult<u64> {
    CLOCK.with(|clock| {
        let (before, after, remaining) = clock.get();
        clock.set((before, after, remaining.saturating_sub(1)));
        Ok(if remaining == 0 { after } else { before })
    })
}

fn clock_runtime(before: u64, after: u64, remaining: usize) -> Runtime {
    CLOCK.with(|clock| clock.set((before, after, remaining)));
    Runtime {
        clock: controlled_clock,
        ..Runtime::default()
    }
}

fn faulty_factory(
    passki: &Passki,
    stored: &StoredPasskey,
) -> RegistrationResult<(AuthenticationChallenge, AuthenticationState)> {
    let case = FACTORY_CASE.with(std::cell::Cell::get);
    if case == 0 {
        return Err(RegistrationError::Unavailable);
    }
    let (mut options, mut state) = authentication_factory(passki, stored)?;
    match case {
        1 => options.rp_id.push('x'),
        2 => options.user_verification = UserVerificationRequirement::Preferred,
        3 => state.user_verification = UserVerificationRequirement::Preferred,
        4 => options.allow_credentials.clear(),
        5 => state.allowed_credentials.clear(),
        6 => state.allowed_credentials.push(vec![0; 32]),
        7 => options.allow_credentials[0].id = encode(&[0; 32]),
        8 => state.challenge.pop().map(|_| ()).unwrap(),
        9 => options.challenge = encode(&[0; 32]),
        _ => options.allow_credentials[0].type_ = "invalid",
    }
    Ok((options, state))
}

#[test]
fn factory_failure_and_public_private_policy_disagreement_refuse() {
    for case in 0..11 {
        let (verifier, proof, registration) = fixture(9);
        let commitment = registration.commitment();
        let (context, reserved) = verifier
            .reserve_confirmation(registration, &proof, DRAFT)
            .unwrap();
        let binding = ConfirmationBinding {
            proposal: [1; 32],
            registration: commitment,
            context,
        };
        FACTORY_CASE.with(|value| value.set(case));
        assert!(verifier
            .start_confirmation_using(
                reserved,
                &proof,
                DRAFT,
                binding,
                Runtime::default(),
                faulty_factory
            )
            .is_err());
    }
}

#[test]
fn reservation_and_finish_refuse_expiry_rollback_and_elapsed_original_deadline() {
    for case in 0..3 {
        let (verifier, proof, mut registration) = fixture(9);
        let now = proof.challenge.issued_at;
        let runtime = match case {
            0 => clock_runtime(proof.challenge.expires_at, proof.challenge.expires_at, 0),
            1 => clock_runtime(now - 1, now - 1, 0),
            _ => {
                registration.deadline = Instant::now();
                clock_runtime(now, now, 0)
            }
        };
        assert!(verifier
            .reserve_confirmation_using(registration, &proof, DRAFT, runtime)
            .is_err());
    }
    for after_verification in [false, true] {
        let (verifier, proof, request, pending) = pending(9);
        let now = pending.binding.context.issued_at;
        let expiry = proof.challenge.expires_at;
        let runtime = clock_runtime(now, expiry, if after_verification { 2 } else { 0 });
        let assertion = serde_json::to_vec(&assertion(&request, 10, 0x1d)).unwrap();
        let signature = signing()
            .sign(&decode(&request.signing_message, 128).unwrap())
            .to_bytes();
        assert!(verifier
            .finish_confirmation_using(
                pending,
                &proof,
                DRAFT,
                request.binding,
                FinalEvidence {
                    device_signature: &signature,
                    assertion: &assertion
                },
                runtime
            )
            .is_err());
    }
}

#[test]
fn reserved_expiry_and_monotonic_deadline_never_renew() {
    let (verifier, proof, registration) = fixture(9);
    let expiry = registration.context.expires_at;
    let deadline = registration.deadline;
    let (context, reserved) = verifier
        .reserve_confirmation(registration, &proof, DRAFT)
        .unwrap();
    assert_eq!(context.expires_at, expiry);
    assert_eq!(reserved.registration.deadline, deadline);
    assert_ne!(context.nonce, reserved.registration.context.nonce);
}

#[test]
fn independent_vector_matches_digest_message_and_complete_owned_flow() {
    fn registration_clock() -> RegistrationResult<u64> {
        Ok(1_600_000_002)
    }
    fn confirmation_clock() -> RegistrationResult<u64> {
        Ok(1_600_000_004)
    }
    fn registration_entropy(bytes: &mut [u8]) -> RegistrationResult<()> {
        bytes.fill(0x44);
        Ok(())
    }
    fn confirmation_entropy(bytes: &mut [u8]) -> RegistrationResult<()> {
        bytes.fill(0x77);
        Ok(())
    }
    let vector: Json = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/data/final-confirmation-vector.json"
    )))
    .unwrap();
    let registration_vector = &vector["registration_fixture"];
    let candidate = crate::enrollment_proof::decode_hex::<251>(
        registration_vector["candidate_hex"].as_str().unwrap(),
    )
    .unwrap();
    let mut offset = b"vivac.device.enrollment-proof.v1\0".len();
    let mut take = |length: usize| {
        let field = &candidate[offset..offset + length];
        offset += length;
        field
    };
    let text = |bytes: &[u8]| std::str::from_utf8(bytes).unwrap().to_owned();
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let proof = Proof {
        challenge: crate::enrollment_proof::Challenge {
            version: 1,
            organization_id: text(take(26)),
            principal_id: text(take(26)),
            device_id: text(take(26)),
            credential_id: text(take(26)),
            public_key: hex(take(32)),
            operation_id: text(take(26)),
            base_revision: u64::from_be_bytes(take(8).try_into().unwrap()),
            nonce: hex(take(32)),
            issued_at: u64::from_be_bytes(take(8).try_into().unwrap()),
            expires_at: u64::from_be_bytes(take(8).try_into().unwrap()),
        },
        signature: registration_vector["proof_signature"]
            .as_str()
            .unwrap()
            .into(),
        status: "possession_only".into(),
        authority: "not_checked".into(),
        challenge_use: "not_checked".into(),
    };
    assert_eq!(proof.challenge.canonical_bytes().unwrap(), candidate);
    let draft = crate::enrollment_proof::decode_hex::<32>(
        registration_vector["review_commitment"].as_str().unwrap(),
    )
    .unwrap();
    let verifier = RegistrationVerifier::new(
        registration_vector["rp_id"].as_str().unwrap(),
        registration_vector["origin"].as_str().unwrap(),
    )
    .unwrap();
    let registration_runtime = Runtime {
        clock: registration_clock,
        entropy: registration_entropy,
        ..Runtime::default()
    };
    let (registration_request, pending_registration) = verifier
        .start_using(&proof, draft, registration_runtime)
        .unwrap();
    assert_eq!(
        hex(&decode(&registration_request.options.challenge, 32).unwrap()),
        registration_vector["challenge_commitment"]
            .as_str()
            .unwrap()
    );
    let raw_registration = registration_vector["registration_json"]
        .as_str()
        .unwrap()
        .as_bytes();
    let registration = verifier
        .finish_using(
            pending_registration,
            &proof,
            draft,
            raw_registration,
            registration_runtime,
        )
        .unwrap();
    let original_commitment = registration.commitment();
    let original_deadline = registration.deadline;
    assert_eq!(
        hex(&original_commitment),
        registration_vector["record_commitment"].as_str().unwrap()
    );
    let confirmation_runtime = Runtime {
        clock: confirmation_clock,
        entropy: confirmation_entropy,
        ..Runtime::default()
    };
    let (context, reserved) = verifier
        .reserve_confirmation_using(registration, &proof, draft, confirmation_runtime)
        .unwrap();
    assert_eq!(
        hex(&context.nonce),
        vector["context"]["nonce"].as_str().unwrap()
    );
    assert_eq!(
        context.issued_at,
        vector["context"]["issued_at"].as_u64().unwrap()
    );
    assert_eq!(
        context.expires_at,
        vector["context"]["expires_at"].as_u64().unwrap()
    );
    let binding = ConfirmationBinding {
        proposal: crate::enrollment_proof::decode_hex::<32>(
            vector["final_proposal_commitment"].as_str().unwrap(),
        )
        .unwrap(),
        registration: original_commitment,
        context,
    };
    let expected_digest =
        crate::enrollment_proof::decode_hex::<32>(vector["challenge_hex"].as_str().unwrap())
            .unwrap();
    assert_eq!(
        digest_parts(&proof, draft, &verifier.rp, &verifier.origin, binding).unwrap(),
        expected_digest
    );
    assert_eq!(
        hex(&signing_message(&expected_digest)),
        vector["signing_message_hex"].as_str().unwrap()
    );
    let (request, pending) = verifier
        .start_confirmation_using(
            reserved,
            &proof,
            draft,
            binding,
            confirmation_runtime,
            authentication_factory,
        )
        .unwrap();
    assert_eq!(
        decode(&request.options.challenge, 32).unwrap(),
        expected_digest
    );
    assert_eq!(pending.state.challenge, expected_digest);
    assert_eq!(
        hex(&decode(&request.signing_message, 128).unwrap()),
        vector["signing_message_hex"].as_str().unwrap()
    );
    let signature =
        crate::enrollment_proof::decode_hex::<64>(vector["final_signature_hex"].as_str().unwrap())
            .unwrap();
    let raw_assertion = vector["assertion_json"].as_str().unwrap().as_bytes();
    let result = verifier
        .finish_confirmation_using(
            pending,
            &proof,
            draft,
            binding,
            FinalEvidence {
                device_signature: &signature,
                assertion: raw_assertion,
            },
            confirmation_runtime,
        )
        .unwrap();
    assert_eq!(result.counter(), 8);
    assert!(!result.backup_state());
    assert_eq!(result.binding(), binding);
    assert_eq!(result.assertion(), raw_assertion);
    assert_eq!(result.device_signature(), signature);
    assert_eq!(result.registration().response, raw_registration);
    assert_eq!(result.registration().commitment(), original_commitment);
    assert_eq!(result.registration().deadline, original_deadline);
    assert_eq!(result.registration().descriptor.counter, 7);
}
