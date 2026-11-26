use super::*;
use base64ct::{Base64UrlUnpadded, Encoding};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

fn vector() -> serde_json::Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/data/additional-admission-vector.json"
    )))
    .unwrap()
}
fn unhex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    (0..value.len() / 2)
        .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}
fn bytes(vector: &serde_json::Value, key: &str) -> Vec<u8> {
    unhex(vector[key].as_str().unwrap())
}
fn id(value: u64) -> String {
    format!("{value:026}")
}
fn organization() -> OrganizationId {
    OrganizationId::parse(&id(1)).unwrap()
}
fn principal() -> PrincipalId {
    PrincipalId::parse(&id(2)).unwrap()
}
fn device(value: u64) -> DeviceId {
    DeviceId::parse(&id(value)).unwrap()
}
fn credential(value: u64) -> CredentialId {
    CredentialId::parse(&id(value)).unwrap()
}
fn old_key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 1: published fixture seed.
    SigningKey::from_bytes(
        &unhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
            .try_into()
            .unwrap(),
    )
}
fn passkey_key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 2: published fixture seed.
    SigningKey::from_bytes(
        &unhex("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb")
            .try_into()
            .unwrap(),
    )
}
fn fixture() -> (AdmissionAuthority, EnrollmentAssertionVerifier, Checkpoint) {
    let vector = vector();
    let mut passkey = passki::StoredPasskey::default();
    passkey.credential_id = vec![0x21; 32];
    passkey.public_key = bytes(&vector, "cose");
    passkey.algorithm = -8;
    let state = PublicPasskeyState::new_fixture(organization(), principal(), passkey).unwrap();
    assert_eq!(hex(&state.commitment()), vector["state0"].as_str().unwrap());
    let known = KnownDevice::new(
        organization(),
        principal(),
        device(3),
        credential(4),
        old_key().verifying_key().to_bytes(),
    )
    .unwrap();
    let authority = AdmissionAuthority::bootstrap_fixture(
        organization(),
        principal(),
        principal(),
        vec![known],
        &[FixturePasskeyAssociation {
            device: device(3),
            credential: credential(4),
            state,
        }],
    )
    .unwrap();
    let verifier = EnrollmentAssertionVerifier::new("vivac.test", "https://vivac.test").unwrap();
    let checkpoint = Checkpoint {
        revision: 0,
        digest: bytes(&vector, "checkpoint0").try_into().unwrap(),
    };
    (authority, verifier, checkpoint)
}
fn next(checkpoint: Checkpoint, request: &AdmissionRequest) -> Checkpoint {
    let mut digest = Sha256::new();
    digest.update(checkpoint.digest);
    digest.update(request.canonical_bytes().unwrap());
    digest.update(request.signature());
    Checkpoint {
        revision: checkpoint.revision + 1,
        digest: digest.finalize().into(),
    }
}
fn granted() -> (AdmissionAuthority, EnrollmentAssertionVerifier, Checkpoint) {
    let vector = vector();
    let (mut authority, verifier, checkpoint) = fixture();
    let policy = AdmissionRequest::parse_canonical(
        &bytes(&vector, "policy_hex"),
        bytes(&vector, "policy_signature").try_into().unwrap(),
    )
    .unwrap();
    assert_eq!(
        authority
            .apply_at(&policy, checkpoint, &verifier)
            .unwrap()
            .revision,
        1
    );
    let checkpoint = next(checkpoint, &policy);
    assert_eq!(
        hex(&checkpoint.digest),
        vector["checkpoint1"].as_str().unwrap()
    );
    (authority, verifier, checkpoint)
}

#[derive(Debug, Eq, PartialEq)]
struct Snapshot {
    revision: u64,
    state: [u8; 32],
    bindings: usize,
    receipts: usize,
    candidate: BTreeSet<[u8; 32]>,
    ceremony: BTreeSet<[u8; 32]>,
}
fn snapshot(authority: &AdmissionAuthority) -> Snapshot {
    Snapshot {
        revision: authority.revision(),
        state: authority.passkeys.values().next().unwrap().commitment(),
        bindings: authority.registry.bindings.len(),
        receipts: authority.enrollments.len(),
        candidate: authority.candidate_nonces.clone(),
        ceremony: authority.ceremony_nonces.clone(),
    }
}

fn signed_assertion(challenge: [u8; 32], counter: u32) -> Vec<u8> {
    let client = serde_json::to_vec(&serde_json::json!({"type":"webauthn.get", "challenge": Base64UrlUnpadded::encode_string(&challenge), "origin":"https://vivac.test", "crossOrigin":false})).unwrap();
    let mut authenticator = Sha256::digest(b"vivac.test").to_vec();
    authenticator.push(0x05);
    authenticator.extend_from_slice(&counter.to_be_bytes());
    let mut signed = authenticator.clone();
    signed.extend_from_slice(&Sha256::digest(&client));
    let raw = Base64UrlUnpadded::encode_string(&[0x21; 32]);
    serde_json::to_vec(&serde_json::json!({"id":raw.clone(),"rawId":raw,"type":"public-key","response":{"clientDataJSON":Base64UrlUnpadded::encode_string(&client),"authenticatorData":Base64UrlUnpadded::encode_string(&authenticator),"signature":Base64UrlUnpadded::encode_string(&passkey_key().sign(&signed).to_bytes()),"userHandle":Base64UrlUnpadded::encode_string(principal().as_str().as_bytes())}})).unwrap()
}

fn record(
    checkpoint: Checkpoint,
    index: u8,
    candidate_nonce: u8,
    ceremony_nonce: u8,
    counter: u32,
) -> AdmissionRecord {
    let key = SigningKey::from_bytes(&[index; 32]);
    let now = 1_600_000_002;
    let proof = Proof::signed(
        Challenge {
            version: 1,
            organization_id: organization().as_str().into(),
            principal_id: principal().as_str().into(),
            device_id: id(100 + u64::from(index)),
            credential_id: id(200 + u64::from(index)),
            public_key: hex(&key.verifying_key().to_bytes()),
            operation_id: id(1000 + u64::from(index)),
            base_revision: checkpoint.revision,
            nonce: hex(&[candidate_nonce; 32]),
            issued_at: now - 2,
            expires_at: now + 298,
        },
        &key,
        now,
    )
    .unwrap();
    let policy = registered_policy_commitment(&organization(), &principal(), checkpoint);
    let message = existing_client_message(
        &organization(),
        &principal(),
        &device(3),
        &credential(4),
        checkpoint,
        policy,
        &proof.challenge,
    )
    .unwrap();
    let endorsement = ExistingClientEndorsement {
        device: device(3),
        credential: credential(4),
        signature: old_key().sign(&message).to_bytes(),
    };
    let context_digest = additional_context_digest(
        &organization(),
        &principal(),
        checkpoint,
        policy,
        &proof.challenge,
        &endorsement,
    )
    .unwrap();
    let context = AssertionContext {
        nonce: [ceremony_nonce; 32],
        issued_at: now - 1,
        expires_at: now + 298,
    };
    let challenge = crate::approval::additional_commitment(
        &proof.challenge,
        &organization(),
        &principal(),
        &[0x21; 32],
        "vivac.test",
        "https://vivac.test",
        &context,
        context_digest,
    )
    .unwrap();
    let witness =
        AdditionalWitness::from_untrusted(context, &signed_assertion(challenge, counter)).unwrap();
    AdmissionRecord {
        proof,
        endorsement,
        witness,
        accepted_at: now,
    }
}

#[test]
fn enrollment_record_has_closed_fixed_bounds() {
    assert_eq!(ADMISSION_DOMAIN.len(), 37);
    assert_eq!(ADMISSION_HEADER, 526);
    assert_eq!(MAX_ADMISSION_BYTES, 16910);
    for length in [0, 226, 526, 16911] {
        assert!(AdmissionRecord::parse_canonical(&vec![0; length]).is_err());
    }
}

#[test]
fn independent_archive_vector_verifies_and_advances_state_and_checkpoint() {
    let vector = vector();
    let (mut authority, verifier, checkpoint) = granted();
    let encoded = bytes(&vector, "record_hex");
    assert_eq!(encoded.len(), 1085);
    assert_eq!(
        hex(&Sha256::digest(&encoded)),
        vector["record_sha256"].as_str().unwrap()
    );
    let record = AdmissionRecord::parse_canonical(&encoded).unwrap();
    assert_eq!(record.canonical_bytes().unwrap(), encoded);
    let request = AdmissionRequest::Enroll(Box::new(record));
    assert_eq!(
        authority.apply_at(&request, checkpoint, &verifier).unwrap(),
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 2
        }
    );
    assert_eq!(
        hex(&next(checkpoint, &request).digest),
        vector["checkpoint2"].as_str().unwrap()
    );
    let state = authority
        .passkey_state(&organization(), &principal(), &device(3), &credential(4))
        .unwrap();
    assert_eq!(hex(&state.commitment()), vector["state1"].as_str().unwrap());
    assert!(authority
        .known_device(&organization(), &principal(), &device(20), &credential(21))
        .is_ok());
    assert!(authority
        .passkey_state(&organization(), &principal(), &device(20), &credential(21))
        .is_err());
}

#[test]
fn reconciliation_uses_retained_acceptance_time_for_expired_historical_proofs() {
    let vector = vector();
    let (mut authority, verifier, checkpoint) = granted();
    let record = AdmissionRecord::parse_canonical(&bytes(&vector, "record_hex")).unwrap();
    let proof = record.proof.clone();
    let endorsement = record.endorsement.clone();
    assert!(proof
        .verify(crate::enrollment_proof::current_time().unwrap())
        .is_err());
    authority
        .apply_at(
            &AdmissionRequest::Enroll(Box::new(record)),
            checkpoint,
            &verifier,
        )
        .unwrap();
    let before = snapshot(&authority);
    assert_eq!(
        authority
            .reconcile_additional_enrollment(&proof, &endorsement)
            .unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 2
        }
    );
    assert_eq!(snapshot(&authority), before);
}

#[test]
fn archive_decoder_refuses_truncation_extra_bytes_domains_and_lengths() {
    let vector = vector();
    let original = bytes(&vector, "record_hex");
    for length in 0..original.len() {
        assert!(AdmissionRecord::parse_canonical(&original[..length]).is_err());
    }
    let mut trailing = original.clone();
    trailing.push(0);
    assert!(AdmissionRecord::parse_canonical(&trailing).is_err());
    for offset in [0, 35, 37, 68, 94, 120, 146, 204, 378, 525] {
        let mut changed = original.clone();
        changed[offset] ^= 0x80;
        assert!(
            AdmissionRecord::parse_canonical(&changed).is_err(),
            "offset {offset}"
        );
    }
    let mut missing = original.clone();
    missing[524] = 0;
    missing[525] = 0;
    assert!(AdmissionRecord::parse_canonical(&missing).is_err());
    assert!(AdmissionRequest::parse_canonical(&original, [0; 64]).is_err());
}

#[test]
fn invalid_assertions_proofs_bindings_and_checkpoints_refuse_without_mutation() {
    let vector = vector();
    for change in [
        "proof",
        "endorsement",
        "assertion",
        "checkpoint",
        "owner",
        "binding",
        "accepted_time",
    ] {
        let (mut authority, verifier, mut checkpoint) = granted();
        let mut record = AdmissionRecord::parse_canonical(&bytes(&vector, "record_hex")).unwrap();
        match change {
            "proof" => record.proof.signature.replace_range(..2, "00"),
            "endorsement" => record.endorsement.signature[0] ^= 1,
            "assertion" => {
                record.witness =
                    AdditionalWitness::from_untrusted(record.witness.context(), b"{}").unwrap()
            }
            "checkpoint" => checkpoint.digest[0] ^= 1,
            "owner" => record.proof.challenge.principal_id = id(8),
            "binding" => record.endorsement.device = device(9),
            "accepted_time" => record.accepted_at = record.witness.context().expires_at,
            _ => unreachable!(),
        }
        let before = snapshot(&authority);
        assert!(
            authority
                .apply_at(
                    &AdmissionRequest::Enroll(Box::new(record)),
                    checkpoint,
                    &verifier
                )
                .is_err(),
            "{change}"
        );
        assert_eq!(snapshot(&authority), before, "{change}");
    }
}

#[test]
fn zero_counter_still_consumes_both_nonces_and_exact_retry_does_not_renew_state() {
    let (mut authority, verifier, checkpoint) = granted();
    let first = record(checkpoint, 70, 11, 22, 0);
    let first_request = AdmissionRequest::Enroll(Box::new(first.clone()));
    authority
        .apply_at(&first_request, checkpoint, &verifier)
        .unwrap();
    let current = next(checkpoint, &first_request);
    assert_eq!(
        (
            authority.passkeys.values().next().unwrap().counter(),
            authority.passkeys.values().next().unwrap().generation()
        ),
        (0, 1)
    );
    let before = snapshot(&authority);
    assert_eq!(
        authority
            .apply_at(&first_request, current, &verifier)
            .unwrap()
            .admission,
        Admission::Historical
    );
    assert_eq!(snapshot(&authority), before);
    for (candidate_nonce, ceremony_nonce) in [(11, 33), (44, 22)] {
        let repeated = record(current, 71, candidate_nonce, ceremony_nonce, 0);
        let (context, state) = authority.check_record_context(&repeated, current).unwrap();
        assert!(verifier
            .replay_recorded_additional(
                &repeated.proof,
                &state,
                &repeated.witness,
                context,
                repeated.accepted_at
            )
            .is_ok());
        assert!(authority
            .apply_at(
                &AdmissionRequest::Enroll(Box::new(repeated)),
                current,
                &verifier
            )
            .is_err());
        assert_eq!(snapshot(&authority), before);
    }
}

#[test]
fn enrollment_quota_is_hard_without_eviction_and_keeps_historical_receipts() {
    let (mut authority, verifier, mut checkpoint) = granted();
    let mut first = None;
    for index in 0..64u8 {
        let request = AdmissionRequest::Enroll(Box::new(record(
            checkpoint,
            70 + index,
            index,
            100 + index,
            0,
        )));
        authority.apply_at(&request, checkpoint, &verifier).unwrap();
        checkpoint = next(checkpoint, &request);
        if first.is_none() {
            first = Some(request);
        }
    }
    assert_eq!(authority.enrollments.len(), 64);
    assert_eq!(authority.registry.bindings.len(), 65);
    assert_eq!(authority.passkeys.values().next().unwrap().generation(), 64);
    let before = snapshot(&authority);
    assert!(authority
        .validate_additional_candidate(
            &organization(),
            &principal(),
            &device(999),
            &credential(998),
            SigningKey::from_bytes(&[200; 32])
                .verifying_key()
                .to_bytes()
        )
        .is_err());
    let request = AdmissionRequest::Enroll(Box::new(record(checkpoint, 134, 64, 164, 0)));
    assert!(authority.apply_at(&request, checkpoint, &verifier).is_err());
    assert_eq!(snapshot(&authority), before);
    assert_eq!(
        authority
            .apply_at(&first.unwrap(), checkpoint, &verifier)
            .unwrap()
            .revision,
        2
    );
    assert_eq!(snapshot(&authority), before);
}

#[test]
fn policy_revocation_and_enrollment_share_one_receipt_bound() {
    let (mut authority, verifier, checkpoint) = granted();
    let first = AdmissionRequest::Enroll(Box::new(record(checkpoint, 70, 11, 22, 0)));
    authority.apply_at(&first, checkpoint, &verifier).unwrap();
    let mut checkpoint = next(checkpoint, &first);
    for index in 0..1022u64 {
        let request = DeviceRequestV2 {
            organization: organization(),
            actor: principal(),
            device: device(3),
            credential: credential(4),
            operation: OperationId::parse(&id(5000 + index)).unwrap(),
            base_revision: checkpoint.revision,
            action: V2Action::SetOwnAdditionalDevice {
                principal: principal(),
                enabled: true,
            },
        };
        let request = AdmissionRequest::Policy(SignedRequestV2 {
            signature: old_key().sign(&request.canonical_bytes()).to_bytes(),
            request,
        });
        authority.apply_at(&request, checkpoint, &verifier).unwrap();
        checkpoint = next(checkpoint, &request);
    }
    assert_eq!(authority.count(), 1024);
    let before = snapshot(&authority);
    let request = AdmissionRequest::Enroll(Box::new(record(checkpoint, 71, 12, 23, 0)));
    assert!(authority.apply_at(&request, checkpoint, &verifier).is_err());
    assert_eq!(snapshot(&authority), before);
    assert_eq!(
        authority
            .apply_at(&first, checkpoint, &verifier)
            .unwrap()
            .admission,
        Admission::Historical
    );
}
