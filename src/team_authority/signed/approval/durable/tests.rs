use super::*;
use crate::approval::tests::{fixture as assertion_fixture, response};
use ed25519_dalek::{Signer, SigningKey};

fn fixture() -> (
    EnrollmentAssertionVerifier,
    PublicPasskeyState,
    SigningKey,
    Proof,
) {
    let (verifier, binding, key, proof) = assertion_fixture();
    let state = PublicPasskeyState::new_fixture(
        binding.organization.clone(),
        binding.principal.clone(),
        binding.passkey,
    )
    .unwrap();
    let mut challenge = proof.challenge;
    challenge.principal_id = state.principal().as_str().into();
    let proof = Proof::signed(challenge, &key, system_time().unwrap()).unwrap();
    (verifier, state, key, proof)
}

fn live_witness(
    verifier: &EnrollmentAssertionVerifier,
    state: &PublicPasskeyState,
    key: &SigningKey,
    proof: &Proof,
) -> (AdditionalWitness, PublicPasskeyState) {
    let (request, pending) = verifier
        .start_recorded_additional(proof, state, [0x37; 32])
        .unwrap();
    let bytes = response(&request, &state.binding().unwrap(), key);
    verifier
        .finish_recorded_additional(
            pending,
            proof,
            state,
            &bytes,
            [0x37; 32],
            system_time().unwrap(),
        )
        .unwrap()
}

fn changed_response(
    request: &AssertionRequest,
    state: &PublicPasskeyState,
    key: &SigningKey,
    flags: u8,
    counter: u32,
) -> Vec<u8> {
    let mut input: serde_json::Value =
        serde_json::from_slice(&response(request, &state.binding().unwrap(), key)).unwrap();
    let client = decode(
        input["response"]["clientDataJSON"].as_str().unwrap(),
        MAX_CLIENT_DATA_BYTES,
    )
    .unwrap();
    let mut data = decode(input["response"]["authenticatorData"].as_str().unwrap(), 37).unwrap();
    data[32] = flags;
    data[33..].copy_from_slice(&counter.to_be_bytes());
    let mut signed = data.clone();
    signed.extend_from_slice(&Sha256::digest(client));
    input["response"]["authenticatorData"] = encode(&data).into();
    input["response"]["signature"] = encode(&key.sign(&signed).to_bytes()).into();
    serde_json::to_vec(&input).unwrap()
}

#[test]
fn recorded_live_and_replayed_updates_match_without_mutating_the_input_state() {
    let (verifier, state, key, proof) = fixture();
    let before = state.commitment();
    let (witness, updated) = live_witness(&verifier, &state, &key, &proof);
    let replayed = verifier
        .replay_recorded_additional(
            &proof,
            &state,
            &witness,
            [0x37; 32],
            witness.context().issued_at,
        )
        .unwrap();
    assert_eq!(state.commitment(), before);
    assert_eq!(
        (state.counter(), state.generation(), state.backup_state()),
        (0, 0, false)
    );
    assert_eq!(
        (
            updated.counter(),
            updated.generation(),
            updated.backup_state()
        ),
        (1, 1, false)
    );
    assert_eq!(updated.commitment(), replayed.commitment());
    assert_eq!(state.credential_id(), &[0x21; 32]);
    assert_eq!(state.organization().as_str(), "00000000000000000000000001");
    assert_eq!(state.principal().as_str(), "00000000000000000000000002");
}

fn historical_clock() -> AssertionResult<u64> {
    Ok(1_000_000)
}

#[test]
fn historical_verification_uses_the_recorded_time_and_does_not_refresh_expiry() {
    let (verifier, state, key, mut proof) = fixture();
    let accepted_at = historical_clock().unwrap();
    proof.challenge.issued_at = accepted_at;
    proof.challenge.expires_at = accepted_at + 300;
    proof = Proof::signed(proof.challenge, &key, accepted_at).unwrap();
    assert!(matches!(
        verifier.start_recorded_additional(&proof, &state, [0x37; 32]),
        Err(AssertionError::Refused)
    ));
    let binding = state.binding().unwrap();
    let (request, pending) = verifier
        .start_scoped_using(
            &proof,
            &binding,
            historical_clock,
            supported_cpu,
            system_ceremony,
            AssertionScope::AdditionalDevice([0x37; 32]),
        )
        .unwrap();
    let bytes = response(&request, &binding, &key);
    let owned = OwnedAdditionalChallenge {
        binding,
        pending,
        state: state.commitment(),
    };
    let (witness, updated) = verifier
        .finish_recorded_additional(owned, &proof, &state, &bytes, [0x37; 32], accepted_at)
        .unwrap();
    let replayed = verifier
        .replay_recorded_additional(&proof, &state, &witness, [0x37; 32], accepted_at)
        .unwrap();
    assert_eq!(updated.commitment(), replayed.commitment());
    assert!(matches!(
        verifier.replay_recorded_additional(
            &proof,
            &state,
            &witness,
            [0x37; 32],
            system_time().unwrap()
        ),
        Err(AssertionError::Refused)
    ));
}

#[test]
fn witness_bounds_are_checked_before_copying_untrusted_assertions() {
    let context = AssertionContext {
        nonce: [0; 32],
        issued_at: 1,
        expires_at: 301,
    };
    for length in [0, MAX_ASSERTION_BYTES + 1] {
        assert!(matches!(
            AdditionalWitness::from_untrusted(context, &vec![0; length]),
            Err(AssertionError::Refused)
        ));
    }
    let witness =
        AdditionalWitness::from_untrusted(context, &vec![0; MAX_ASSERTION_BYTES]).unwrap();
    assert_eq!(witness.assertion().len(), MAX_ASSERTION_BYTES);
    for (issued_at, expires_at) in [(1, 1), (2, 1), (1, 302)] {
        assert!(matches!(
            AdditionalWitness::from_untrusted(
                AssertionContext {
                    issued_at,
                    expires_at,
                    ..context
                },
                b"x"
            ),
            Err(AssertionError::Refused)
        ));
    }
}

#[test]
fn fixture_state_rejects_unsupported_metadata_instead_of_discarding_it() {
    let (_, state, _, _) = fixture();
    for case in 0..7 {
        let mut passkey = state.passkey.clone();
        match case {
            0 => passkey.aaguid[0] = 1,
            1 => passkey.attestation_type = passki::AttestationType::Basic,
            2 => passkey.transports.push(passki::AuthenticatorTransport::Usb),
            3 => passkey.rk = Some(false),
            4 => passkey.large_blob_supported = Some(false),
            5 => {
                passkey.cred_protect =
                    Some(passki::CredentialProtectionPolicy::UserVerificationRequired)
            }
            6 => passkey.min_pin_length = Some(4),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                PublicPasskeyState::new_fixture(
                    state.organization.clone(),
                    state.principal.clone(),
                    passkey
                ),
                Err(AssertionError::Refused)
            ),
            "{case}"
        );
    }
}

#[test]
fn fixture_state_rejects_invalid_sizes_algorithms_and_backup_flags() {
    let (_, state, _, _) = fixture();
    for case in 0..6 {
        let mut passkey = state.passkey.clone();
        match case {
            0 => passkey.credential_id.clear(),
            1 => passkey.credential_id.resize(MAX_CREDENTIAL_BYTES + 1, 0),
            2 => passkey.public_key.clear(),
            3 => passkey.public_key.resize(MAX_PUBLIC_KEY_BYTES + 1, 0),
            4 => passkey.algorithm = -257,
            5 => passkey.bs = true,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                PublicPasskeyState::new_fixture(
                    state.organization.clone(),
                    state.principal.clone(),
                    passkey
                ),
                Err(AssertionError::Refused)
            ),
            "{case}"
        );
    }
}

#[test]
fn public_state_commitments_cover_identity_key_counter_flags_and_generation() {
    let (_, state, _, _) = fixture();
    let original = state.commitment();
    for case in 0..9 {
        let mut changed = state.clone();
        match case {
            0 => {
                changed.organization = OrganizationId::parse("00000000000000000000000009").unwrap()
            }
            1 => changed.principal = PrincipalId::parse("00000000000000000000000009").unwrap(),
            2 => changed.passkey.credential_id[0] ^= 1,
            3 => changed.passkey.public_key[0] ^= 1,
            4 => changed.passkey.algorithm = -7,
            5 => changed.passkey.counter = 1,
            6 => changed.passkey.be = true,
            7 => {
                changed.passkey.be = true;
                changed.passkey.bs = true;
            }
            8 => changed.generation = 1,
            _ => unreachable!(),
        }
        assert_ne!(changed.commitment(), original, "{case}");
    }
}

#[test]
fn a_live_challenge_refuses_changed_public_state_before_verification() {
    for case in 0..6 {
        let (verifier, state, key, proof) = fixture();
        let (request, pending) = verifier
            .start_recorded_additional(&proof, &state, [0x37; 32])
            .unwrap();
        let bytes = response(&request, &state.binding().unwrap(), &key);
        let mut changed = state.clone();
        match case {
            0 => changed.passkey.counter += 1,
            1 => changed.generation += 1,
            2 => changed.passkey.credential_id[0] ^= 1,
            3 => changed.passkey.public_key[0] ^= 1,
            4 => changed.passkey.be = true,
            5 => changed.principal = PrincipalId::parse("00000000000000000000000009").unwrap(),
            _ => unreachable!(),
        }
        let before = changed.commitment();
        assert!(matches!(
            verifier.finish_recorded_additional(
                pending,
                &proof,
                &changed,
                &bytes,
                [0x37; 32],
                system_time().unwrap()
            ),
            Err(AssertionError::Refused)
        ));
        assert_eq!(changed.commitment(), before);
    }
}

#[test]
fn replay_refuses_changed_context_signature_credential_key_and_owner() {
    let (verifier, state, key, proof) = fixture();
    let (witness, _) = live_witness(&verifier, &state, &key, &proof);
    for case in 0..9 {
        let mut changed = witness.clone();
        let mut current = state.clone();
        match case {
            0 => changed.context.nonce[0] ^= 1,
            1 => changed.context.issued_at = changed.context.issued_at.saturating_sub(1),
            2 => changed.context.expires_at += 1,
            3 => changed.assertion[0] ^= 1,
            4 => current.passkey.credential_id[0] ^= 1,
            5 => {
                let last = current.passkey.public_key.len() - 1;
                current.passkey.public_key[last] ^= 1;
            }
            6 => {
                current.organization = OrganizationId::parse("00000000000000000000000009").unwrap()
            }
            7 => current.principal = PrincipalId::parse("00000000000000000000000009").unwrap(),
            8 => {
                let mut input: serde_json::Value =
                    serde_json::from_slice(&changed.assertion).unwrap();
                let mut signature =
                    decode(input["response"]["signature"].as_str().unwrap(), 128).unwrap();
                signature[0] ^= 1;
                input["response"]["signature"] = encode(&signature).into();
                changed.assertion = serde_json::to_vec(&input).unwrap();
            }
            _ => unreachable!(),
        }
        let before = current.commitment();
        assert!(
            matches!(
                verifier.replay_recorded_additional(
                    &proof,
                    &current,
                    &changed,
                    [0x37; 32],
                    witness.context.issued_at
                ),
                Err(AssertionError::Refused)
            ),
            "{case}"
        );
        assert_eq!(current.commitment(), before);
    }
    assert!(matches!(
        verifier.replay_recorded_additional(
            &proof,
            &state,
            &witness,
            [0x38; 32],
            witness.context.issued_at
        ),
        Err(AssertionError::Refused)
    ));
}

#[test]
fn replay_refuses_times_outside_both_signed_windows() {
    let (verifier, state, key, proof) = fixture();
    let (witness, _) = live_witness(&verifier, &state, &key, &proof);
    for accepted_at in [
        witness.context.issued_at - 1,
        witness.context.expires_at,
        witness.context.expires_at + 1,
    ] {
        assert!(matches!(
            verifier.replay_recorded_additional(&proof, &state, &witness, [0x37; 32], accepted_at),
            Err(AssertionError::Refused)
        ));
    }
}

#[test]
fn nonzero_counter_replay_refuses_a_witness_after_the_state_advanced() {
    let (verifier, state, key, proof) = fixture();
    let (witness, updated) = live_witness(&verifier, &state, &key, &proof);
    assert!(matches!(
        verifier.replay_recorded_additional(
            &proof,
            &updated,
            &witness,
            [0x37; 32],
            witness.context.issued_at
        ),
        Err(AssertionError::Refused)
    ));
}

#[test]
fn zero_counter_witnesses_require_separate_durable_consumption() {
    let (verifier, state, key, proof) = fixture();
    let (request, pending) = verifier
        .start_recorded_additional(&proof, &state, [0x37; 32])
        .unwrap();
    let bytes = changed_response(&request, &state, &key, 0x05, 0);
    let (witness, updated) = verifier
        .finish_recorded_additional(
            pending,
            &proof,
            &state,
            &bytes,
            [0x37; 32],
            system_time().unwrap(),
        )
        .unwrap();
    assert_eq!((updated.counter(), updated.generation()), (0, 1));
    let again = verifier
        .replay_recorded_additional(
            &proof,
            &updated,
            &witness,
            [0x37; 32],
            witness.context.issued_at,
        )
        .unwrap();
    assert_eq!((again.counter(), again.generation()), (0, 2));
}

#[test]
fn backup_state_is_derived_from_the_verified_authenticator_flags() {
    let (verifier, mut state, key, proof) = fixture();
    state.passkey.be = true;
    let (request, pending) = verifier
        .start_recorded_additional(&proof, &state, [0x37; 32])
        .unwrap();
    let bytes = changed_response(&request, &state, &key, 0x1d, 1);
    let (witness, updated) = verifier
        .finish_recorded_additional(
            pending,
            &proof,
            &state,
            &bytes,
            [0x37; 32],
            system_time().unwrap(),
        )
        .unwrap();
    assert!(updated.backup_state());
    let replayed = verifier
        .replay_recorded_additional(
            &proof,
            &state,
            &witness,
            [0x37; 32],
            witness.context.issued_at,
        )
        .unwrap();
    assert_eq!(updated.commitment(), replayed.commitment());
    assert!(!state.backup_state());
}

#[test]
fn generation_overflow_refuses_without_updating_the_public_state() {
    let (verifier, mut state, key, proof) = fixture();
    state.generation = u64::MAX;
    let (request, pending) = verifier
        .start_recorded_additional(&proof, &state, [0x37; 32])
        .unwrap();
    let bytes = response(&request, &state.binding().unwrap(), &key);
    let before = state.commitment();
    assert!(matches!(
        verifier.finish_recorded_additional(
            pending,
            &proof,
            &state,
            &bytes,
            [0x37; 32],
            system_time().unwrap()
        ),
        Err(AssertionError::Refused)
    ));
    assert_eq!(state.commitment(), before);
}

#[test]
fn configuration_commitment_covers_the_exact_canonical_relying_party_and_origin() {
    let first = EnrollmentAssertionVerifier::new("vivac.test", "https://vivac.test").unwrap();
    let same = EnrollmentAssertionVerifier::new("vivac.test", "https://vivac.test:443/").unwrap();
    let other = EnrollmentAssertionVerifier::new("other.test", "https://other.test").unwrap();
    assert_eq!(
        first.configuration_commitment(),
        same.configuration_commitment()
    );
    assert_ne!(
        first.configuration_commitment(),
        other.configuration_commitment()
    );
}

#[test]
fn public_state_and_configuration_match_independent_commitment_vectors() {
    let (verifier, state, _, _) = fixture();
    let hex = |bytes: [u8; 32]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        hex(state.commitment()),
        "2df7d66945d81ef5d9e23bca88efd19ba10cf27573f2121ac6cf97cd91aba16b"
    );
    assert_eq!(
        hex(verifier.configuration_commitment()),
        "8f79a66a47285fc0a296f002b53a68247a6cdbfa9587e6b48d63eef6c6bc774c"
    );
}
