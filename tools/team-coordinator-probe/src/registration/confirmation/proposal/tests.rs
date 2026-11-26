use super::*;
use ed25519_dalek::Signer;
use serde_json::{json, Value as Json};

const RP: &str = "vivac.test";
const ORIGIN: &str = "https://vivac.test";

fn clock() -> RegistrationResult<u64> {
    Ok(1_600_000_002)
}

pub(super) fn runtime() -> Runtime {
    Runtime {
        clock,
        ..Runtime::default()
    }
}

fn confirmation_clock() -> RegistrationResult<u64> {
    Ok(1_600_000_004)
}

fn expired_clock() -> RegistrationResult<u64> {
    Ok(1_600_000_300)
}

fn before_clock() -> RegistrationResult<u64> {
    Ok(1_599_999_999)
}

fn before_registration_clock() -> RegistrationResult<u64> {
    Ok(1_600_000_001)
}

fn before_confirmation_clock() -> RegistrationResult<u64> {
    Ok(1_600_000_003)
}

fn registration_entropy(bytes: &mut [u8]) -> RegistrationResult<()> {
    bytes.fill(0x44);
    Ok(())
}

fn confirmation_entropy(bytes: &mut [u8]) -> RegistrationResult<()> {
    bytes.fill(0x77);
    Ok(())
}

pub(super) fn registration_runtime() -> Runtime {
    Runtime {
        entropy: registration_entropy,
        ..runtime()
    }
}

pub(super) fn confirmation_runtime() -> Runtime {
    Runtime {
        clock: confirmation_clock,
        entropy: confirmation_entropy,
        ..runtime()
    }
}

pub(super) fn signing() -> ed25519_dalek::SigningKey {
    // RFC 8032 section 7.1, test 1; public test material only.
    ed25519_dalek::SigningKey::from_bytes(
        &crate::enrollment_proof::decode_hex::<32>(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        )
        .unwrap(),
    )
}

pub(super) fn input() -> Json {
    let key = signing();
    let proof = Proof::signed(
        crate::enrollment_proof::Challenge {
            version: 1,
            organization_id: "00000000000000000000000001".into(),
            principal_id: "00000000000000000000000002".into(),
            device_id: "00000000000000000000000003".into(),
            credential_id: "00000000000000000000000004".into(),
            public_key: key
                .verifying_key()
                .to_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            operation_id: "00000000000000000000000005".into(),
            base_revision: 0,
            nonce: "11".repeat(32),
            issued_at: 1_600_000_000,
            expires_at: 1_600_000_300,
        },
        &key,
        1_600_000_002,
    )
    .unwrap();
    json!({
        "version": 1,
        "profile": "initial-proposal-probe-v1",
        "purpose": "initial-organization",
        "candidate": proof,
        "coordinator": {"endpoint": "https://vivac.test", "spki_sha256": "99".repeat(32)},
        "webauthn": {"rp_id": "vivac.test", "origin": "https://vivac.test",
            "registration_profile": "passki-0.5-registration-probe-v1", "user_verification": "required"},
        "grants": {"capabilities": ["grant-project-access", "manage-members"],
            "delegation": "organization-model-v2", "enrollment_policy_administrator": false,
            "own_additional_device": true},
        "project_grants": [], "governance_grants": [],
        "recovery": "not-established", "content_key_custody": "not-established"
    })
}

#[test]
fn a_valid_complete_draft_is_accepted_without_authority() {
    let raw = serde_json::to_vec(&input()).unwrap();
    let draft = Draft::parse_using(&raw, runtime()).unwrap();
    let pretty = serde_json::to_vec_pretty(&input()).unwrap();
    let same = Draft::parse_using(&pretty, runtime()).unwrap();
    assert_eq!(draft.canonical(), same.canonical());
    assert_eq!(draft.commitment(), same.commitment());
    assert!(draft
        .canonical()
        .starts_with(b"{\"version\":1,\"profile\":"));
    assert!(!draft.canonical().ends_with(b"\n"));
    assert_eq!(draft.proof().challenge.base_revision, 0);
    let value: Json = serde_json::from_slice(draft.canonical()).unwrap();
    assert_eq!(value["candidate"], input()["candidate"]);
    assert!(value.get("approved").is_none());
}

#[test]
fn a_v2_draft_binds_acceptance_identities_before_registration() {
    let mut value = input();
    value["version"] = json!(2);
    value["profile"] = json!("initial-acceptance-probe-v1");
    value["acceptance"] = json!({
        "organization_authority_id": "00000000000000000000000040",
        "organization_authority_public_key":
            "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "principal_acceptance": "candidate-signing-key"
    });
    assert!(Draft::parse_using(&serde_json::to_vec(&value).unwrap(), runtime()).is_ok());
}

fn parse(value: &Json) -> RegistrationResult<Draft> {
    Draft::parse_using(&serde_json::to_vec(value).unwrap(), runtime())
}

fn resign(value: &mut Json) {
    let proof: Proof = serde_json::from_value(value["candidate"].clone()).unwrap();
    value["candidate"] =
        serde_json::to_value(Proof::signed(proof.challenge, &signing(), 1_600_000_002).unwrap())
            .unwrap();
}

pub(super) fn registration_for(draft: &Draft) -> (RegistrationVerifier, UnconfirmedRegistration) {
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    let (request, pending) = verifier
        .start_using(draft.proof(), draft.commitment(), registration_runtime())
        .unwrap();
    let integer = |value: i32| Value::Integer(value.into());
    let cose = Value::Map(vec![
        (integer(1), integer(1)),
        (integer(3), integer(-8)),
        (integer(-1), integer(6)),
        (
            integer(-2),
            Value::Bytes(signing().verifying_key().to_bytes().to_vec()),
        ),
    ]);
    let mut auth = Sha256::digest(RP.as_bytes()).to_vec();
    auth.push(0x5d);
    auth.extend_from_slice(&9_u32.to_be_bytes());
    auth.extend_from_slice(&[0x45; 16]);
    auth.extend_from_slice(&32_u16.to_be_bytes());
    auth.extend_from_slice(&[0x31; 32]);
    ciborium::into_writer(&cose, &mut auth).unwrap();
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
    let raw = serde_json::to_vec(&json!({
        "id": encode(&[0x31;32]), "rawId": encode(&[0x31;32]), "type": "public-key",
        "response": {"clientDataJSON": encode(&serde_json::to_vec(&json!({
            "type":"webauthn.create", "challenge":request.options.challenge,
            "origin":ORIGIN, "crossOrigin":false})).unwrap()),
            "attestationObject":encode(&attestation), "transports":["usb","internal"]},
        "clientExtensionResults":{}, "authenticatorAttachment":"cross-platform"
    }))
    .unwrap();
    let registration = verifier
        .finish_using(
            pending,
            draft.proof(),
            draft.commitment(),
            &raw,
            registration_runtime(),
        )
        .unwrap();
    (verifier, registration)
}

fn ready() -> (RegistrationVerifier, ReadyProposal) {
    let draft = parse(&input()).unwrap();
    let (verifier, registration) = registration_for(&draft);
    let ready = verifier
        .prepare_proposal_using(draft, registration, confirmation_runtime())
        .unwrap();
    (verifier, ready)
}

fn pending() -> (RegistrationVerifier, FinalAssertionRequest, PendingProposal) {
    let (verifier, ready) = ready();
    let (request, pending) = verifier
        .start_proposal_using(ready, confirmation_runtime())
        .unwrap();
    (verifier, request, pending)
}

pub(super) fn evidence(request: &FinalAssertionRequest) -> ([u8; 64], Vec<u8>) {
    let client = serde_json::to_vec(&json!({
        "type":"webauthn.get", "challenge":request.options.challenge,
        "origin":ORIGIN, "crossOrigin":false
    }))
    .unwrap();
    let mut auth = Sha256::digest(RP.as_bytes()).to_vec();
    auth.push(0x1d);
    auth.extend_from_slice(&10_u32.to_be_bytes());
    let mut message = auth.clone();
    message.extend_from_slice(&Sha256::digest(&client));
    let assertion = serde_json::to_vec(&json!({
        "id":encode(&[0x31;32]), "rawId":encode(&[0x31;32]), "type":"public-key",
        "response":{"clientDataJSON":encode(&client), "authenticatorData":encode(&auth),
            "signature":encode(&signing().sign(&message).to_bytes())}, "clientExtensionResults":{}
    }))
    .unwrap();
    let device = signing()
        .sign(&decode(&request.signing_message, 128).unwrap())
        .to_bytes();
    (device, assertion)
}

#[test]
fn required_draft_fields_have_no_defaults_or_unknown_extensions() {
    for path in [
        "",
        "/coordinator",
        "/webauthn",
        "/grants",
        "/candidate",
        "/candidate/challenge",
    ] {
        let source = input();
        let object = if path.is_empty() {
            &source
        } else {
            source.pointer(path).unwrap()
        };
        for field in object.as_object().unwrap().keys() {
            let mut value = input();
            let target = if path.is_empty() {
                &mut value
            } else {
                value.pointer_mut(path).unwrap()
            };
            target.as_object_mut().unwrap().remove(field);
            assert!(parse(&value).is_err(), "missing {path}/{field}");
        }
        let mut value = input();
        let target = if path.is_empty() {
            &mut value
        } else {
            value.pointer_mut(path).unwrap()
        };
        target
            .as_object_mut()
            .unwrap()
            .insert("approved".into(), json!(true));
        assert!(parse(&value).is_err(), "unknown field at {path}");
    }
    let raw = serde_json::to_string(&input()).unwrap();
    for modified in [
        format!("{{\"version\":1,{}", &raw[1..]),
        raw.replacen(
            "\"endpoint\":",
            "\"endpoint\":\"https://vivac.test\",\"endpoint\":",
            1,
        ),
        raw.replacen("\"challenge\":{", "\"challenge\":{\"version\":1,", 1),
        raw.replacen(
            "\"own_additional_device\":",
            "\"own_additional_device\":false,\"own_additional_device\":",
            1,
        ),
    ] {
        assert!(Draft::parse_using(modified.as_bytes(), runtime()).is_err());
    }
}

#[test]
fn unsupported_policies_types_and_noncanonical_grants_refuse() {
    let cases = [
        ("/version", json!(2)),
        ("/profile", json!("future")),
        ("/purpose", json!("additional-device")),
        ("/coordinator/endpoint", json!("https://vivac.test/")),
        ("/coordinator/endpoint", json!("https://other.test")),
        ("/coordinator/spki_sha256", json!("AA".repeat(32))),
        ("/coordinator/spki_sha256", json!("99".repeat(31))),
        ("/webauthn/origin", json!("https://vivac.test/")),
        ("/webauthn/rp_id", json!("other.test")),
        ("/webauthn/registration_profile", json!("unknown")),
        ("/webauthn/user_verification", json!("preferred")),
        (
            "/grants/capabilities",
            json!(["manage-members", "manage-members"]),
        ),
        (
            "/grants/capabilities",
            json!(["manage-projects", "manage-members"]),
        ),
        ("/grants/capabilities", json!(["administrator"])),
        ("/grants/capabilities", json!(["a", "b", "c", "d", "e"])),
        ("/grants/delegation", json!("none")),
        ("/grants/enrollment_policy_administrator", json!("false")),
        ("/grants/own_additional_device", Json::Null),
        ("/project_grants", json!(["project"])),
        ("/governance_grants", json!(["accept-norms"])),
        ("/recovery", json!("enabled")),
        ("/content_key_custody", json!("server")),
    ];
    for (path, replacement) in cases {
        let mut value = input();
        *value.pointer_mut(path).unwrap() = replacement;
        assert!(parse(&value).is_err(), "accepted {path}");
    }
    let mut empty = input();
    empty["grants"]["capabilities"] = json!([]);
    let parsed = parse(&empty).unwrap();
    let output: Json = serde_json::from_slice(parsed.canonical()).unwrap();
    assert_eq!(output["grants"]["capabilities"], json!([]));
}

#[test]
fn draft_bounds_invalid_json_and_expired_or_wrong_candidate_refuse() {
    let raw = serde_json::to_vec(&input()).unwrap();
    let mut padded = raw.clone();
    padded.resize(MAX_DRAFT, b' ');
    assert!(Draft::parse_using(&padded, runtime()).is_ok());
    padded.push(b' ');
    assert!(Draft::parse_using(&padded, runtime()).is_err());
    let invalid_inputs: [&[u8]; 4] = [b"", b"\xff", b"{}x", b"null"];
    for invalid in invalid_inputs {
        assert!(Draft::parse_using(invalid, runtime()).is_err());
    }
    for clock in [expired_clock, before_clock] {
        assert!(Draft::parse_using(&raw, Runtime { clock, ..runtime() }).is_err());
    }
    for case in 0..4 {
        let mut value = input();
        match case {
            0 => value["candidate"]["signature"] = json!("00".repeat(64)),
            1 => value["candidate"]["status"] = json!("approved"),
            2 => value["candidate"]["authority"] = json!("verified"),
            _ => {
                value["candidate"]["challenge"]["base_revision"] = json!(1);
                resign(&mut value);
            }
        }
        assert!(parse(&value).is_err());
    }
}

#[test]
fn changed_bound_draft_fields_change_commitment_or_refuse() {
    let original = parse(&input()).unwrap().commitment();
    let changes = [
        ("/coordinator/spki_sha256", json!("98".repeat(32))),
        ("/grants/capabilities", json!([])),
        ("/grants/enrollment_policy_administrator", json!(true)),
        ("/grants/own_additional_device", json!(false)),
        ("/recovery", json!("enabled")),
        (
            "/candidate/challenge/organization_id",
            json!("00000000000000000000000009"),
        ),
        (
            "/candidate/challenge/principal_id",
            json!("00000000000000000000000009"),
        ),
        (
            "/candidate/challenge/device_id",
            json!("00000000000000000000000009"),
        ),
        (
            "/candidate/challenge/credential_id",
            json!("00000000000000000000000009"),
        ),
        (
            "/candidate/challenge/operation_id",
            json!("00000000000000000000000009"),
        ),
        ("/candidate/challenge/nonce", json!("12".repeat(32))),
        ("/candidate/challenge/expires_at", json!(1_600_000_299)),
    ];
    for (path, replacement) in changes {
        let mut value = input();
        *value.pointer_mut(path).unwrap() = replacement;
        if path.starts_with("/candidate/challenge/") {
            resign(&mut value);
        }
        if let Ok(changed) = parse(&value) {
            assert_ne!(changed.commitment(), original, "unbound {path}");
        }
    }
    let mut alternate = input();
    alternate["coordinator"]["endpoint"] = json!("https://other.test");
    alternate["webauthn"]["origin"] = json!("https://other.test");
    alternate["webauthn"]["rp_id"] = json!("other.test");
    assert_ne!(parse(&alternate).unwrap().commitment(), original);
}

#[test]
fn complete_owned_flow_preserves_raw_registration_and_initial_metadata() {
    let draft = parse(&input()).unwrap();
    let canonical = draft.canonical().to_vec();
    let draft_commitment = draft.commitment();
    let (verifier, registration) = registration_for(&draft);
    let response = registration.response.clone();
    let deadline = registration.deadline;
    let original_descriptor = format!("{:?}", registration.descriptor);
    let original_commitment = registration.commitment;
    let ready = verifier
        .prepare_proposal_using(draft, registration, confirmation_runtime())
        .unwrap();
    assert_eq!(ready.reserved.registration.deadline, deadline);
    assert_eq!(ready.context().expires_at, 1_600_000_300);
    let packet = ready.packet().to_vec();
    let commitment = ready.commitment();
    let value: Json = serde_json::from_slice(&packet).unwrap();
    assert_eq!(
        value["draft"],
        serde_json::from_slice::<Json>(&canonical).unwrap()
    );
    assert_eq!(value["draft_commitment"], hex(&draft_commitment));
    assert_eq!(value["registration"]["response"], encode(&response));
    assert_eq!(value["registration"]["descriptor"]["counter"], 9);
    assert_eq!(value["registration"]["descriptor"]["backup_eligible"], true);
    assert_eq!(value["registration"]["descriptor"]["backup_state"], true);
    assert_eq!(
        value["registration"]["descriptor"]["transports"],
        json!(["usb", "internal"])
    );
    assert_eq!(
        value["registration"]["descriptor"]["attachment"],
        "cross-platform"
    );
    assert!(value.get("commitment").is_none());
    let (request, pending) = verifier
        .start_proposal_using(ready, confirmation_runtime())
        .unwrap();
    assert_eq!(pending.packet(), packet.as_slice());
    assert_eq!(pending.commitment(), commitment);
    assert_eq!(request.binding.proposal, commitment);
    assert_eq!(request.binding.registration, original_commitment);
    let (device, assertion) = evidence(&request);
    let confirmed = verifier
        .finish_proposal_using(pending, &device, &assertion, confirmation_runtime())
        .unwrap();
    assert_eq!(confirmed.packet(), packet.as_slice());
    assert_eq!(confirmed.confirmation().registration().response, response);
    assert_eq!(
        format!("{:?}", confirmed.confirmation().registration().descriptor),
        original_descriptor
    );
    assert_eq!(
        confirmed.confirmation().registration().descriptor.counter,
        9
    );
    assert_eq!(confirmed.confirmation().counter(), 10);
    assert_eq!(confirmed.confirmation().assertion(), assertion.as_slice());
    assert_eq!(
        confirmed.confirmation().device_signature(),
        device.as_slice()
    );
}

#[test]
fn preparation_refuses_substitution_and_original_deadline_expiry() {
    for case in 0..10 {
        let mut draft = parse(&input()).unwrap();
        let (verifier, mut registration) = registration_for(&draft);
        match case {
            0 => {
                let mut changed = input();
                changed["coordinator"]["spki_sha256"] = json!("98".repeat(32));
                draft = parse(&changed).unwrap();
            }
            1 => registration.commitment[0] ^= 1,
            2 => registration.response.push(b' '),
            3 => registration.descriptor.counter += 1,
            4 => registration.stored.counter += 1,
            5 => registration.rp.push('x'),
            6 => registration.context.nonce[0] ^= 1,
            7 => registration.deadline = Instant::now(),
            8 => draft.canonical.push(b' '),
            _ => registration.proof[0] ^= 1,
        }
        assert!(verifier
            .prepare_proposal_using(draft, registration, confirmation_runtime())
            .is_err());
    }
    let draft = parse(&input()).unwrap();
    let (_, registration) = registration_for(&draft);
    let other = RegistrationVerifier::new("other.test", "https://other.test").unwrap();
    assert!(other
        .prepare_proposal_using(draft, registration, confirmation_runtime())
        .is_err());
}

#[test]
fn ready_packet_and_context_cannot_be_replaced_or_extended() {
    for case in 0..10 {
        let (verifier, mut ready) = ready();
        match case {
            0 => ready.packet.push(b' '),
            1 => ready.binding.proposal[0] ^= 1,
            2 => ready.binding.registration[0] ^= 1,
            3 => ready.binding.context.nonce[0] ^= 1,
            4 => ready.binding.context.issued_at += 1,
            5 => ready.reserved.context.nonce[0] ^= 1,
            6 => ready.reserved.registration.descriptor.counter += 1,
            7 => ready.reserved.registration.deadline = Instant::now(),
            8 => ready.packet.resize(MAX_PROPOSAL + 1, b' '),
            _ => {
                ready.draft.value.grants.own_additional_device = false;
                ready.draft.canonical = serde_json::to_vec(&ready.draft.value).unwrap();
                ready.draft.commitment = hash(DRAFT_DOMAIN, &ready.draft.canonical);
            }
        }
        assert!(verifier
            .start_proposal_using(ready, confirmation_runtime())
            .is_err());
    }
}

#[test]
fn pending_packet_and_missing_or_invalid_final_evidence_refuse() {
    for case in 0..9 {
        let (verifier, request, mut pending) = pending();
        let (mut device, mut assertion) = evidence(&request);
        match case {
            0 => pending.packet.push(b' '),
            1 => pending.binding.proposal[0] ^= 1,
            2 => pending.pending.binding.proposal[0] ^= 1,
            3 => pending.pending.registration.deadline = Instant::now(),
            4 => device[0] ^= 1,
            5 => assertion.clear(),
            6 => pending.packet.resize(MAX_PROPOSAL + 1, b' '),
            7 => pending.pending.state.challenge[0] ^= 1,
            _ => pending.pending.registration.descriptor.public_key.push(0),
        }
        assert!(verifier
            .finish_proposal_using(pending, &device, &assertion, confirmation_runtime())
            .is_err());
    }
}

#[test]
fn wall_clock_expiry_and_rollback_refuse_at_every_live_stage() {
    for clock in [expired_clock, before_clock] {
        let draft = parse(&input()).unwrap();
        let (verifier, registration) = registration_for(&draft);
        let changed = Runtime {
            clock,
            ..confirmation_runtime()
        };
        assert!(verifier
            .prepare_proposal_using(draft, registration, changed)
            .is_err());
        let (verifier, ready) = ready();
        assert!(verifier.start_proposal_using(ready, changed).is_err());
        let (verifier, request, pending) = pending();
        let (device, assertion) = evidence(&request);
        assert!(verifier
            .finish_proposal_using(pending, &device, &assertion, changed)
            .is_err());
    }
    let draft = parse(&input()).unwrap();
    let (verifier, registration) = registration_for(&draft);
    assert!(verifier
        .prepare_proposal_using(
            draft,
            registration,
            Runtime {
                clock: before_registration_clock,
                ..confirmation_runtime()
            }
        )
        .is_err());
    let (verifier, ready) = ready();
    assert!(verifier
        .start_proposal_using(
            ready,
            Runtime {
                clock: before_confirmation_clock,
                ..confirmation_runtime()
            }
        )
        .is_err());
    let (verifier, request, pending) = pending();
    let (device, assertion) = evidence(&request);
    assert!(verifier
        .finish_proposal_using(
            pending,
            &device,
            &assertion,
            Runtime {
                clock: before_confirmation_clock,
                ..confirmation_runtime()
            }
        )
        .is_err());
}

#[test]
fn preparation_rechecks_original_monotonic_deadline_after_packet_construction() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn expiring_monotonic() -> Instant {
        if CALLS.fetch_add(1, Ordering::SeqCst) >= 3 {
            Instant::now() + Duration::from_secs(301)
        } else {
            Instant::now()
        }
    }
    let draft = parse(&input()).unwrap();
    let (verifier, registration) = registration_for(&draft);
    CALLS.store(0, Ordering::SeqCst);
    assert!(verifier
        .prepare_proposal_using(
            draft,
            registration,
            Runtime {
                monotonic: expiring_monotonic,
                ..confirmation_runtime()
            }
        )
        .is_err());
    assert!(CALLS.load(Ordering::SeqCst) >= 4);
}

#[test]
fn independent_vector_matches_draft_packet_and_complete_live_flow() {
    let vector: Json = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/data/initial-proposal-vector.json"
    )))
    .unwrap();
    let draft_json = vector["draft_json"].as_str().unwrap().as_bytes();
    let draft = Draft::parse_using(draft_json, registration_runtime()).unwrap();
    assert_eq!(draft.canonical(), draft_json);
    assert_eq!(hex(&draft.commitment()), vector["draft_commitment"]);
    let verifier = RegistrationVerifier::new(RP, ORIGIN).unwrap();
    let (request, pending) = verifier
        .start_using(draft.proof(), draft.commitment(), registration_runtime())
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
            registration_runtime(),
        )
        .unwrap();
    assert_eq!(
        hex(&registration.commitment()),
        vector["registration_commitment"]
    );
    let deadline = registration.deadline;
    let ready = verifier
        .prepare_proposal_using(draft, registration, confirmation_runtime())
        .unwrap();
    assert_eq!(ready.reserved.registration.deadline, deadline);
    assert_eq!(
        ready.packet(),
        vector["proposal_json"].as_str().unwrap().as_bytes()
    );
    assert_eq!(hex(&ready.commitment()), vector["proposal_commitment"]);
    assert_eq!(hex(&ready.context().nonce), vector["context"]["nonce"]);
    let packet = ready.packet().to_vec();
    let (request, pending) = verifier
        .start_proposal_using(ready, confirmation_runtime())
        .unwrap();
    assert_eq!(
        hex(&decode(&request.options.challenge, 32).unwrap()),
        vector["challenge_hex"]
    );
    assert_eq!(
        hex(&decode(&request.signing_message, 128).unwrap()),
        vector["signing_message_hex"]
    );
    let signature =
        crate::enrollment_proof::decode_hex::<64>(vector["final_signature_hex"].as_str().unwrap())
            .unwrap();
    let confirmed = verifier
        .finish_proposal_using(
            pending,
            &signature,
            vector["assertion_json"].as_str().unwrap().as_bytes(),
            confirmation_runtime(),
        )
        .unwrap();
    assert_eq!(confirmed.packet(), packet.as_slice());
    assert_eq!(
        confirmed.confirmation().registration().descriptor.counter,
        7
    );
    assert_eq!(confirmed.confirmation().counter(), 8);
}
