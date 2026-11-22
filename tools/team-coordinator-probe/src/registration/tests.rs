use super::*;
use serde_json::{json, Value as Json};

const RP: &str = "vivac.test";
const ORIGIN: &str = "https://vivac.test";
const REVIEW: [u8; 32] = [0x52; 32];

fn public_proof() -> Proof {
    let now = crate::enrollment_proof::current_time().unwrap();
    let signing = ed25519_dalek::SigningKey::from_bytes(
        &hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
            .try_into()
            .unwrap(),
    );
    Proof::signed(
        crate::enrollment_proof::Challenge {
            version: 1,
            organization_id: "00000000000000000000000001".into(),
            principal_id: "00000000000000000000000003".into(),
            device_id: "00000000000000000000000004".into(),
            credential_id: "00000000000000000000000005".into(),
            public_key: signing
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
        &signing,
        now,
    )
    .unwrap()
}
fn fixture() -> (
    RegistrationVerifier,
    Proof,
    RegistrationRequest,
    PendingRegistration,
) {
    let proof = public_proof();
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    let (request, pending) = verifier.start(&proof, REVIEW).unwrap();
    (verifier, proof, request, pending)
}
fn integer(number: i32) -> Value {
    Value::Integer(number.into())
}
fn cbor(value: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}
fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|part| u8::from_str_radix(std::str::from_utf8(part).unwrap(), 16).unwrap())
        .collect()
}
fn key(algorithm: i32) -> Value {
    if algorithm == -8 {
        // RFC 8032, section 7.1, test 1 public key.
        Value::Map(vec![
            (integer(1), integer(1)),
            (integer(3), integer(-8)),
            (integer(-1), integer(6)),
            (
                integer(-2),
                Value::Bytes(hex(
                    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                )),
            ),
        ])
    } else {
        // RFC 6979, appendix A.2.5 public point.
        Value::Map(vec![
            (integer(1), integer(2)),
            (integer(3), integer(-7)),
            (integer(-1), integer(1)),
            (
                integer(-2),
                Value::Bytes(hex(
                    "60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6",
                )),
            ),
            (
                integer(-3),
                Value::Bytes(hex(
                    "7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299",
                )),
            ),
        ])
    }
}
fn auth(algorithm: i32) -> Vec<u8> {
    let mut bytes = Sha256::digest(RP.as_bytes()).to_vec();
    bytes.push(0x5d);
    bytes.extend_from_slice(&9_u32.to_be_bytes());
    bytes.extend_from_slice(&[0x45; 16]);
    bytes.extend_from_slice(&32_u16.to_be_bytes());
    bytes.extend_from_slice(&[0x31; 32]);
    bytes.extend_from_slice(&cbor(&key(algorithm)));
    bytes
}
fn attestation(auth: Vec<u8>) -> Value {
    Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text("none".into())),
        (Value::Text("authData".into()), Value::Bytes(auth)),
        (Value::Text("attStmt".into()), Value::Map(vec![])),
    ])
}
fn response(request: &RegistrationRequest, algorithm: i32) -> Json {
    json!({"id": encode(&[0x31;32]), "rawId": encode(&[0x31;32]), "type":"public-key",
        "response":{"clientDataJSON":encode(&serde_json::to_vec(&json!({
            "type":"webauthn.create", "challenge":request.options.challenge,
            "origin":ORIGIN, "crossOrigin":false})).unwrap()),
            "attestationObject":encode(&cbor(&attestation(auth(algorithm)))),
            "transports":["usb","hybrid","internal"]},
        "clientExtensionResults":{},"authenticatorAttachment":"cross-platform"})
}
fn finish_json(
    value: &Json,
    fixture: (
        RegistrationVerifier,
        Proof,
        RegistrationRequest,
        PendingRegistration,
    ),
) -> RegistrationResult<UnconfirmedRegistration> {
    let (verifier, proof, _, pending) = fixture;
    verifier.finish(pending, &proof, REVIEW, &serde_json::to_vec(value).unwrap())
}
fn refused(result: RegistrationResult<UnconfirmedRegistration>) {
    assert!(matches!(result, Err(RegistrationError::Refused)));
}
fn change_signature(proof: &mut Proof) {
    let replacement = if proof.signature.starts_with('0') {
        "1"
    } else {
        "0"
    };
    proof.signature.replace_range(..1, replacement);
}
fn replace_attestation(value: &mut Json, object: Value) {
    value["response"]["attestationObject"] = json!(encode(&cbor(&object)));
}

#[test]
fn ed25519_registration_retains_unconfirmed_metadata_and_original_response() {
    let fixture = fixture();
    let value = response(&fixture.2, -8);
    let digest = fixture.3.digest;
    let context = fixture.2.context;
    let raw = serde_json::to_vec(&value).unwrap();
    let result = finish_json(&value, fixture).unwrap();
    let descriptor = result.descriptor();
    assert_eq!(descriptor.aaguid, [0x45; 16]);
    assert_eq!(descriptor.counter, 9);
    assert!(descriptor.backup_eligible && descriptor.backup_state);
    assert_eq!(descriptor.algorithm, -8);
    assert_eq!(descriptor.public_key, cbor(&key(-8)));
    assert_eq!(descriptor.credential, vec![0x31; 32]);
    assert_eq!(descriptor.transports, ["usb", "hybrid", "internal"]);
    assert_eq!(descriptor.attachment.as_deref(), Some("cross-platform"));
    assert_eq!(descriptor.opaque_user_handle, b"00000000000000000000000003");
    assert_eq!(result.response, raw);
    assert_eq!(result.context.nonce, context.nonce);
    let mut hash = Sha256::new();
    hash.update(b"vivac.passkey.registration-record.v1\0");
    hash.update(digest);
    hash.update((raw.len() as u32).to_be_bytes());
    hash.update(raw);
    assert_eq!(result.commitment(), <[u8; 32]>::from(hash.finalize()));
}
#[test]
fn p256_registration_validates_the_public_point_without_admitting_authority() {
    let fixture = fixture();
    let value = response(&fixture.2, -7);
    let result = finish_json(&value, fixture).unwrap();
    assert_eq!(result.descriptor().algorithm, -7);
    assert_eq!(result.descriptor().public_key, cbor(&key(-7)));
}
#[test]
fn creation_options_require_verification_and_discourage_resident_credentials() {
    let (_, _, request, pending) = fixture();
    let options = serde_json::to_value(request.options).unwrap();
    assert_eq!(
        options["pubKeyCredParams"],
        json!([{"type":"public-key","alg":-8},{"type":"public-key","alg":-7}])
    );
    assert_eq!(
        options["authenticatorSelection"]["userVerification"],
        "required"
    );
    assert_eq!(
        options["authenticatorSelection"]["residentKey"],
        "discouraged"
    );
    assert_eq!(options["attestation"], "none");
    assert!(options.get("extensions").is_none());
    assert_eq!(
        decode(options["challenge"].as_str().unwrap(), 32).unwrap(),
        pending.digest
    );
}
#[test]
fn configuration_rejects_insecure_or_inexact_origins() {
    for (rp, origin) in [
        (RP, "http://vivac.test"),
        (RP, "https://other.test"),
        (RP, "https://vivac.test/path"),
        (RP, "https://vivac.test?x=1"),
        (RP, "https://user@vivac.test"),
        ("", "https://vivac.test"),
    ] {
        assert!(matches!(
            RegistrationVerifier::new(rp, origin),
            Err(RegistrationError::Refused)
        ));
    }
}
#[test]
fn pending_registration_binds_review_proof_and_configuration() {
    for case in 0..5 {
        let (verifier, mut proof, request, mut pending) = fixture();
        let value = response(&request, -8);
        let mut review = REVIEW;
        match case {
            0 => review[0] ^= 1,
            1 => change_signature(&mut proof),
            2 => pending.rp.push('x'),
            3 => pending.origin.push('x'),
            _ => pending.state.challenge[0] ^= 1,
        }
        refused(verifier.finish(
            pending,
            &proof,
            review,
            &serde_json::to_vec(&value).unwrap(),
        ));
    }
}
#[test]
fn pending_registration_expires_without_waiting_or_renewing_its_deadline() {
    let (verifier, proof, request, mut pending) = fixture();
    let raw = serde_json::to_vec(&response(&request, -8)).unwrap();
    pending.deadline = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    refused(verifier.finish(pending, &proof, REVIEW, &raw));
}
#[test]
fn pending_registration_rejects_wall_clock_rollback_and_expiry() {
    fn old() -> RegistrationResult<u64> {
        Ok(1)
    }
    fn future() -> RegistrationResult<u64> {
        Ok(u64::MAX)
    }
    for clock in [old as fn() -> RegistrationResult<u64>, future] {
        let (verifier, proof, request, pending) = fixture();
        refused(verifier.finish_using(
            pending,
            &proof,
            REVIEW,
            &serde_json::to_vec(&response(&request, -8)).unwrap(),
            Runtime {
                clock,
                ..Runtime::default()
            },
        ));
    }
}
#[test]
fn response_size_and_base64_must_be_bounded_and_canonical() {
    let (verifier, proof, _, pending) = fixture();
    refused(verifier.finish(pending, &proof, REVIEW, &vec![b' '; MAX_RESPONSE + 1]));
    for field in ["clientDataJSON", "attestationObject"] {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        value["response"][field] =
            json!(format!("{}=", value["response"][field].as_str().unwrap()));
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn duplicate_json_fields_are_rejected_at_every_protocol_level() {
    for case in 0..4 {
        let (verifier, proof, request, pending) = fixture();
        let mut value = response(&request, -8);
        if case == 3 {
            value["response"]["clientDataJSON"] = json!(encode(format!(
                "{{\"type\":\"webauthn.create\",\"type\":\"webauthn.create\",\"challenge\":\"{}\",\"origin\":\"{}\"}}",
                request.options.challenge,ORIGIN).as_bytes()));
        }
        let text = serde_json::to_string(&value).unwrap();
        let text = match case {
            0 => text.replacen("{", "{\"type\":\"public-key\",", 1),
            1 => text.replacen("\"response\":{", "\"response\":{\"transports\":[],", 1),
            2 => text.replacen(
                "\"clientExtensionResults\":{}",
                "\"clientExtensionResults\":{\"x\":1,\"x\":1}",
                1,
            ),
            _ => text,
        };
        refused(verifier.finish(pending, &proof, REVIEW, text.as_bytes()));
    }
}
#[test]
fn unknown_json_fields_are_rejected_before_backend_normalization() {
    for case in 0..3 {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        match case {
            0 => value["unknown"] = json!(true),
            1 => value["response"]["publicKey"] = json!("ignored-by-backend"),
            _ => value["clientExtensionResults"]["credProps"] = json!({"rk":true}),
        }
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn client_data_rejects_wrong_types_origins_challenges_and_embedding() {
    for (field, value) in [
        ("type", json!("webauthn.get")),
        ("origin", json!("https://other.test")),
        ("challenge", json!(encode(&[0; 32]))),
        ("crossOrigin", json!(true)),
        ("crossOrigin", json!(null)),
        ("topOrigin", json!(ORIGIN)),
        ("origin", json!(7)),
    ] {
        let fixture = fixture();
        let mut response = response(&fixture.2, -8);
        let mut client: Json = serde_json::from_slice(
            &decode(
                response["response"]["clientDataJSON"].as_str().unwrap(),
                MAX_COMPONENT,
            )
            .unwrap(),
        )
        .unwrap();
        client[field] = value;
        response["response"]["clientDataJSON"] =
            json!(encode(&serde_json::to_vec(&client).unwrap()));
        refused(finish_json(&response, fixture));
    }
}
#[test]
fn outer_identity_and_credential_substitution_are_rejected() {
    for (field, value) in [
        ("id", json!(encode(&[1; 32]))),
        ("rawId", json!(encode(&[2; 32]))),
        ("type", json!("other")),
    ] {
        let fixture = fixture();
        let mut response = response(&fixture.2, -8);
        response[field] = value;
        refused(finish_json(&response, fixture));
    }
}
#[test]
fn reported_transport_and_attachment_metadata_are_strict() {
    for transports in [
        json!(null),
        json!(["cable"]),
        json!(["future"]),
        json!(["usb", "usb"]),
        json!([
            "usb",
            "nfc",
            "ble",
            "smart-card",
            "hybrid",
            "internal",
            "usb"
        ]),
    ] {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        value["response"]["transports"] = transports;
        refused(finish_json(&value, fixture));
    }
    for attachment in [json!(null), json!("unknown"), json!(5)] {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        value["authenticatorAttachment"] = attachment;
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn omitted_optional_metadata_remains_absent() {
    let fixture = fixture();
    let mut value = response(&fixture.2, -8);
    value
        .as_object_mut()
        .unwrap()
        .remove("authenticatorAttachment");
    value
        .as_object_mut()
        .unwrap()
        .remove("clientExtensionResults");
    value["response"]
        .as_object_mut()
        .unwrap()
        .remove("transports");
    let result = finish_json(&value, fixture).unwrap();
    assert!(result.descriptor().attachment.is_none());
    assert!(result.descriptor().transports.is_empty());
}
#[test]
fn authenticator_flags_require_presence_verification_and_attested_data() {
    for flags in [0x44, 0x41, 0x05, 0xc5, 0x47, 0x65, 0x55] {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        let mut auth = auth(-8);
        auth[32] = flags;
        replace_attestation(&mut value, attestation(auth));
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn authenticator_data_rejects_wrong_relying_party_lengths_and_trailing_bytes() {
    for case in 0..5 {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        let mut auth = auth(-8);
        match case {
            0 => auth[0] ^= 1,
            1 => auth.truncate(54),
            2 => auth[53..55].copy_from_slice(&0_u16.to_be_bytes()),
            3 => auth[53..55].copy_from_slice(&1025_u16.to_be_bytes()),
            _ => auth.push(0),
        }
        replace_attestation(&mut value, attestation(auth));
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn attestation_requires_exact_none_statement_structure() {
    for case in 0..5 {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        let mut object = attestation(auth(-8));
        let map = object.as_map_mut().unwrap();
        match case {
            0 => map[0].1 = Value::Text("packed".into()),
            1 => {
                map[2].1 = Value::Map(vec![(Value::Text("sig".into()), Value::Bytes(vec![0; 64]))])
            }
            2 => {
                map.pop();
            }
            3 => {
                map.push(map[0].clone());
            }
            _ => map[0].0 = Value::Text("unknown".into()),
        }
        replace_attestation(&mut value, object);
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn cbor_rejects_nonminimal_indefinite_trailing_and_excessive_depth() {
    let mut trailing = cbor(&attestation(auth(-8)));
    trailing.push(0);
    let mut indefinite = cbor(&attestation(auth(-8)));
    indefinite[0] = 0xbf;
    indefinite.push(0xff);
    let mut nonminimal = cbor(&attestation(auth(-8)));
    nonminimal.splice(..1, [0xb8, 3]);
    let mut deep = vec![0x81; 10];
    deep.push(0);
    let oversized_map = vec![0xbb, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let oversized_array = vec![0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let oversized_bytes = vec![0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    for bytes in [
        trailing,
        indefinite,
        nonminimal,
        deep,
        oversized_map,
        oversized_array,
        oversized_bytes,
    ] {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        value["response"]["attestationObject"] = json!(encode(&bytes));
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn cose_rejects_duplicate_unknown_and_substituted_algorithm_labels() {
    for case in 0..5 {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        let mut key = key(-8);
        let map = key.as_map_mut().unwrap();
        match case {
            0 => map.push(map[0].clone()),
            1 => map.push((integer(99), integer(1))),
            2 => map[1].1 = integer(-7),
            3 => map[2].1 = integer(1),
            _ => map[3].1 = Value::Bytes(vec![1; 31]),
        }
        let mut auth = auth(-8);
        auth.truncate(87);
        auth.extend_from_slice(&cbor(&key));
        replace_attestation(&mut value, attestation(auth));
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn weak_ed25519_and_invalid_p256_points_are_rejected() {
    for algorithm in [-8, -7] {
        let fixture = fixture();
        let mut value = response(&fixture.2, algorithm);
        let mut key = key(algorithm);
        for (label, value) in key.as_map_mut().unwrap() {
            if label
                .as_integer()
                .and_then(|n| i64::try_from(n).ok())
                .is_some_and(|n| n <= -2)
            {
                *value = Value::Bytes(vec![0; 32]);
            }
        }
        let mut auth = auth(algorithm);
        auth.truncate(87);
        auth.extend_from_slice(&cbor(&key));
        replace_attestation(&mut value, attestation(auth));
        refused(finish_json(&value, fixture));
    }
}
#[test]
fn entropy_cpu_clock_and_factory_failures_are_closed() {
    fn entropy(_: &mut [u8]) -> RegistrationResult<()> {
        Err(RegistrationError::Unavailable)
    }
    fn cpu() -> bool {
        false
    }
    fn clock() -> RegistrationResult<u64> {
        Err(RegistrationError::Unavailable)
    }
    fn factory(
        _: &Passki,
        _: &[u8],
    ) -> RegistrationResult<(RegistrationChallenge, RegistrationState)> {
        Err(RegistrationError::Unavailable)
    }
    let (verifier, proof, _, _) = fixture();
    for runtime in [
        Runtime {
            entropy,
            ..Runtime::default()
        },
        Runtime {
            cpu,
            ..Runtime::default()
        },
        Runtime {
            clock,
            ..Runtime::default()
        },
        Runtime {
            factory,
            ..Runtime::default()
        },
    ] {
        assert!(matches!(
            verifier.start_using(&proof, REVIEW, runtime),
            Err(RegistrationError::Unavailable)
        ));
    }
}
#[test]
fn factory_panics_are_contained_without_creating_pending_state() {
    fn factory(
        _: &Passki,
        _: &[u8],
    ) -> RegistrationResult<(RegistrationChallenge, RegistrationState)> {
        panic!("synthetic registration factory failure")
    }
    let (verifier, proof, _, _) = fixture();
    assert!(matches!(
        verifier.start_using(
            &proof,
            REVIEW,
            Runtime {
                factory,
                ..Runtime::default()
            }
        ),
        Err(RegistrationError::Unavailable)
    ));
}
#[test]
fn backend_descriptor_disagreement_cannot_be_silently_normalized() {
    let fixture = fixture();
    let value = response(&fixture.2, -8);
    let descriptor = parse_response(
        &serde_json::to_vec(&value).unwrap(),
        &fixture.3.digest,
        RP,
        ORIGIN,
    )
    .unwrap();
    let mut stored = StoredPasskey::default();
    stored.credential_id = descriptor.credential.clone();
    stored.public_key = descriptor.public_key.clone();
    stored.algorithm = descriptor.algorithm;
    stored.counter = descriptor.counter;
    stored.aaguid = descriptor.aaguid;
    stored.be = true;
    stored.bs = true;
    stored.transports = serde_json::from_value(json!(descriptor.transports)).unwrap();
    assert_eq!(check_stored(&descriptor, &stored), Ok(()));
    for case in 0..5 {
        let mut changed = stored.clone();
        match case {
            0 => changed.aaguid = [0; 16],
            1 => changed.counter += 1,
            2 => changed.public_key.push(0),
            3 => changed.rk = Some(true),
            _ => changed.transports.clear(),
        }
        assert_eq!(
            check_stored(&descriptor, &changed),
            Err(RegistrationError::Refused)
        );
    }
}
#[test]
fn separate_ceremonies_cannot_exchange_registration_responses() {
    let first = fixture();
    let second = fixture();
    let response = response(&first.2, -8);
    assert_ne!(first.3.digest, second.3.digest);
    refused(finish_json(&response, second));
}
#[test]
fn finish_rechecks_time_after_the_backend_before_returning_a_descriptor() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn clock() -> RegistrationResult<u64> {
        if CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            system_time()
        } else {
            Ok(u64::MAX)
        }
    }
    let (verifier, proof, request, pending) = fixture();
    CALLS.store(0, Ordering::SeqCst);
    refused(verifier.finish_using(
        pending,
        &proof,
        REVIEW,
        &serde_json::to_vec(&response(&request, -8)).unwrap(),
        Runtime {
            clock,
            ..Runtime::default()
        },
    ));
    assert_eq!(CALLS.load(Ordering::SeqCst), 2);
}

#[test]
fn independent_registration_vector_matches_challenge_record_and_metadata() {
    fn clock() -> RegistrationResult<u64> {
        Ok(1_600_000_002)
    }
    fn entropy(bytes: &mut [u8]) -> RegistrationResult<()> {
        bytes.fill(0x44);
        Ok(())
    }
    let vector: Json = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/data/passkey-registration-vector.json"
    )))
    .unwrap();
    let proof = Proof {
        challenge: crate::enrollment_proof::Challenge {
            version: 1,
            organization_id: "00000000000000000000000001".into(),
            principal_id: "00000000000000000000000002".into(),
            device_id: "00000000000000000000000020".into(),
            credential_id: "00000000000000000000000021".into(),
            public_key: "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025".into(),
            operation_id: "00000000000000000000000030".into(),
            base_revision: 1,
            nonce: "11".repeat(32),
            issued_at: 1_600_000_000,
            expires_at: 1_600_000_300,
        },
        signature: vector["proof_signature"].as_str().unwrap().into(),
        status: "possession_only".into(),
        authority: "not_checked".into(),
        challenge_use: "not_checked".into(),
    };
    assert_eq!(
        proof.challenge.canonical_bytes().unwrap(),
        hex(vector["candidate_hex"].as_str().unwrap())
    );
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    let runtime = Runtime {
        clock,
        entropy,
        ..Runtime::default()
    };
    let (request, pending) = verifier.start_using(&proof, [0x33; 32], runtime).unwrap();
    assert_eq!(
        decode(&request.options.challenge, 32).unwrap(),
        hex(vector["challenge_commitment"].as_str().unwrap())
    );
    let raw = vector["registration_json"].as_str().unwrap().as_bytes();
    let result = verifier
        .finish_using(pending, &proof, [0x33; 32], raw, runtime)
        .unwrap();
    assert_eq!(
        result.commitment().to_vec(),
        hex(vector["record_commitment"].as_str().unwrap())
    );
    assert_eq!(result.descriptor().aaguid, [0x55; 16]);
    assert_eq!(result.descriptor().counter, 7);
    assert_eq!(result.descriptor().credential, vec![0x66; 32]);
    assert_eq!(
        result.descriptor().public_key,
        hex(vector["cose_key"].as_str().unwrap())
    );
    assert_eq!(result.descriptor().transports, ["usb", "internal"]);
    assert_eq!(
        result.descriptor().attachment.as_deref(),
        Some("cross-platform")
    );
    assert_eq!(result.response, raw);
}

#[test]
fn invalid_or_expired_proofs_cannot_start_registration() {
    let (verifier, proof, _, _) = fixture();
    for case in 0..3 {
        let mut changed = proof.clone();
        match case {
            0 => changed.status = "approved".into(),
            1 => changed.challenge.expires_at = changed.challenge.issued_at,
            _ => change_signature(&mut changed),
        }
        assert!(matches!(
            verifier.start(&changed, REVIEW),
            Err(RegistrationError::Refused)
        ));
    }
}
#[test]
fn factory_cannot_weaken_required_verification_or_replace_the_user() {
    fn weak(
        passki: &Passki,
        user: &[u8],
    ) -> RegistrationResult<(RegistrationChallenge, RegistrationState)> {
        let (options, mut state) = system_factory(passki, user)?;
        state.user_verification = UserVerificationRequirement::Preferred;
        Ok((options, state))
    }
    fn crossed(
        passki: &Passki,
        user: &[u8],
    ) -> RegistrationResult<(RegistrationChallenge, RegistrationState)> {
        let (mut options, state) = system_factory(passki, user)?;
        options.user.id = encode(&[1; 26]);
        Ok((options, state))
    }
    fn wrong_rp(
        passki: &Passki,
        user: &[u8],
    ) -> RegistrationResult<(RegistrationChallenge, RegistrationState)> {
        let (mut options, state) = system_factory(passki, user)?;
        options.rp.id = "other.test".into();
        Ok((options, state))
    }
    let (verifier, proof, _, _) = fixture();
    for factory in [weak as Factory, crossed, wrong_rp] {
        assert!(matches!(
            verifier.start_using(
                &proof,
                REVIEW,
                Runtime {
                    factory,
                    ..Runtime::default()
                }
            ),
            Err(RegistrationError::Refused)
        ));
    }
}
#[test]
fn unsupported_cpu_refuses_finish_before_public_key_validation() {
    fn cpu() -> bool {
        false
    }
    let (verifier, proof, _, pending) = fixture();
    assert!(matches!(
        verifier.finish_using(
            pending,
            &proof,
            REVIEW,
            b"malformed",
            Runtime {
                cpu,
                ..Runtime::default()
            }
        ),
        Err(RegistrationError::Unavailable)
    ));
}
#[test]
fn component_and_credential_bounds_reject_oversized_inputs() {
    for case in 0..4 {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        match case {
            0 => value["response"]["clientDataJSON"] = json!(encode(&vec![0; MAX_COMPONENT + 1])),
            1 => {
                value["response"]["attestationObject"] = json!(encode(&vec![0; MAX_COMPONENT + 1]))
            }
            2 => {
                let id = encode(&vec![0; MAX_KEY + 1]);
                value["id"] = json!(id);
                value["rawId"] = value["id"].clone();
            }
            _ => {
                let mut bytes = auth(-8);
                bytes.truncate(87);
                bytes.extend_from_slice(&vec![0; MAX_KEY + 1]);
                replace_attestation(&mut value, attestation(bytes));
            }
        }
        refused(finish_json(&value, fixture));
    }
}

#[test]
fn default_registration_api_returns_only_an_unconfirmed_public_descriptor() {
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    let proof = public_proof();
    if !supported_cpu() {
        assert!(matches!(
            verifier.start(&proof, REVIEW),
            Err(RegistrationError::Unavailable)
        ));
        return;
    }
    let (request, pending) = verifier.start(&proof, REVIEW).unwrap();
    let raw = serde_json::to_vec(&response(&request, -8)).unwrap();
    let result = verifier.finish(pending, &proof, REVIEW, &raw).unwrap();
    assert_eq!(result.descriptor().aaguid, [0x45; 16]);
    assert_eq!(result.descriptor().algorithm, -8);
    assert_eq!(result.descriptor().credential, vec![0x31; 32]);
    assert_eq!(
        result.descriptor().transports,
        ["usb", "hybrid", "internal"]
    );
    assert_ne!(result.commitment(), [0; 32]);
    assert_eq!(result.response, raw);
}

#[test]
fn wrapper_rejects_nonempty_none_statement_that_the_backend_accepts() {
    let (verifier, proof, request, pending) = fixture();
    let mut response = response(&request, -8);
    let mut object = attestation(auth(-8));
    object.as_map_mut().unwrap()[2].1 =
        Value::Map(vec![(Value::Text("sig".into()), Value::Bytes(vec![0; 64]))]);
    replace_attestation(&mut response, object);
    let raw = serde_json::to_vec(&response).unwrap();
    let credential: RegistrationCredential = serde_json::from_slice(&raw).unwrap();
    let backend = verifier
        .passki
        .finish_passkey_registration(&credential, &pending.state)
        .unwrap();
    assert_eq!(backend.credential_id, vec![0x31; 32]);
    assert_eq!(backend.attestation_type, AttestationType::None);
    refused(verifier.finish_using(pending, &proof, REVIEW, &raw, Runtime::default()));
}

#[test]
fn chromium_client_data_marker_preserves_original_response_and_commitment() {
    let fixture = fixture();
    let mut value = response(&fixture.2, -8);
    let digest = fixture.3.digest;
    let mut client: Json = serde_json::from_slice(
        &decode(
            value["response"]["clientDataJSON"].as_str().unwrap(),
            MAX_COMPONENT,
        )
        .unwrap(),
    )
    .unwrap();
    let without_marker = serde_json::to_vec(&value).unwrap();
    client["other_keys_can_be_added_here"] =
        json!("do not compare clientDataJSON against a template. See https://goo.gl/yabPex");
    value["response"]["clientDataJSON"] = json!(encode(&serde_json::to_vec(&client).unwrap()));
    let raw = serde_json::to_vec(&value).unwrap();
    let result = finish_json(&value, fixture).unwrap();
    assert_eq!(result.response, raw);
    assert_eq!(result.descriptor().credential, vec![0x31; 32]);
    let mut preimage = b"vivac.passkey.registration-record.v1\0".to_vec();
    preimage.extend(digest);
    preimage.extend((raw.len() as u32).to_be_bytes());
    preimage.extend(&raw);
    let expected: [u8; 32] = Sha256::digest(&preimage).into();
    assert_eq!(result.commitment(), expected);
    preimage.truncate(b"vivac.passkey.registration-record.v1\0".len() + digest.len());
    preimage.extend((without_marker.len() as u32).to_be_bytes());
    preimage.extend(&without_marker);
    let stripped: [u8; 32] = Sha256::digest(&preimage).into();
    assert_ne!(result.commitment(), stripped);
}

#[test]
fn chromium_client_data_marker_rejects_changed_strings_and_nonstring_values() {
    for marker in [
        json!(null),
        json!(false),
        json!(true),
        json!({}),
        json!([]),
        json!(0),
        json!(1.5),
        json!(""),
        json!("do not compare clientDataJSON against a template. See https://goo.gl/yabPex "),
        json!("do not compare clientDataJSON against a template. See https://goo.gl/changed"),
    ] {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        let mut client: Json = serde_json::from_slice(
            &decode(
                value["response"]["clientDataJSON"].as_str().unwrap(),
                MAX_COMPONENT,
            )
            .unwrap(),
        )
        .unwrap();
        client["other_keys_can_be_added_here"] = marker;
        value["response"]["clientDataJSON"] = json!(encode(&serde_json::to_vec(&client).unwrap()));
        refused(finish_json(&value, fixture));
    }
}

#[test]
fn chromium_client_data_marker_rejects_duplicate_fields_without_collapsing_them() {
    let exact = serde_json::to_string(
        "do not compare clientDataJSON against a template. See https://goo.gl/yabPex",
    )
    .unwrap();
    for (first, second) in [
        (exact.as_str(), exact.as_str()),
        ("null", exact.as_str()),
        (exact.as_str(), "null"),
    ] {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        let original = String::from_utf8(
            decode(
                value["response"]["clientDataJSON"].as_str().unwrap(),
                MAX_COMPONENT,
            )
            .unwrap(),
        )
        .unwrap();
        // Assemble duplicate keys as text; a JSON value would collapse them.
        let duplicate = format!(
            "{},\"other_keys_can_be_added_here\":{first},\"other_keys_can_be_added_here\":{second}}}",
            original.strip_suffix('}').unwrap()
        );
        value["response"]["clientDataJSON"] = json!(encode(duplicate.as_bytes()));
        refused(finish_json(&value, fixture));
    }
}

#[test]
fn chromium_client_data_marker_does_not_bypass_client_data_guards() {
    for (field, changed) in [
        ("origin", json!("https://other.test")),
        ("challenge", json!(encode(&[0; 32]))),
        ("crossOrigin", json!(true)),
        ("topOrigin", json!(ORIGIN)),
        ("unknown", json!(false)),
    ] {
        let fixture = fixture();
        let mut value = response(&fixture.2, -8);
        let mut client: Json = serde_json::from_slice(
            &decode(
                value["response"]["clientDataJSON"].as_str().unwrap(),
                MAX_COMPONENT,
            )
            .unwrap(),
        )
        .unwrap();
        client["other_keys_can_be_added_here"] =
            json!("do not compare clientDataJSON against a template. See https://goo.gl/yabPex");
        client[field] = changed;
        value["response"]["clientDataJSON"] = json!(encode(&serde_json::to_vec(&client).unwrap()));
        refused(finish_json(&value, fixture));
    }
}
