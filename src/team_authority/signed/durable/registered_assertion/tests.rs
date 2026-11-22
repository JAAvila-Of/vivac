use super::super::tests::{fd, fixture as authority_fixture, root, TestRoot};
use super::super::{FixtureRegisteredTrust, RegisteredRequest};
use super::*;
use crate::approval::tests::{
    fixture as assertion_fixture, fixture_binding_with_identity, fixture_state, response,
};
use crate::team_authority::signed::v2::registry::{RevocationRequest, SignedRevocation};
use crate::team_authority::signed::v2::{DeviceRequestV2, SignedRequestV2, V2Action};
use crate::team_authority::signed::KnownDevice;
use ed25519_dalek::{Signer, SigningKey};
use std::fs;

fn grant(store: &mut AuthorityStoreRegistered, trust: &FixtureRegisteredTrust, key: &SigningKey) {
    let (_, _, device, credential, _) = trust.base.known[0].binding_fields();
    let request = DeviceRequestV2 {
        organization: trust.base.organization.clone(),
        actor: trust.base.initial_principal.clone(),
        device: device.clone(),
        credential: credential.clone(),
        operation: OperationId::parse("00000000000000000000000010").unwrap(),
        base_revision: store.checkpoint().unwrap().revision,
        action: V2Action::SetOwnAdditionalDevice {
            principal: trust.base.initial_principal.clone(),
            enabled: true,
        },
    };
    store
        .admit(&RegisteredRequest::Policy(SignedRequestV2 {
            signature: key.sign(&request.canonical_bytes()).to_bytes(),
            request,
        }))
        .unwrap();
}

#[test]
fn registered_assertion_uses_current_registry_and_explicit_policy() {
    let root = root();
    let (base, old_key, _) = authority_fixture();
    let trust = FixtureRegisteredTrust {
        policy_admin: base.initial_principal.clone(),
        base,
    };
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    grant(&mut store, &trust, &old_key);
    let (verifier, binding, candidate_key, _) = assertion_fixture();
    let (_, _, device, credential, _) = trust.base.known[0].binding_fields();
    let mut passkey = FixtureRegisteredPasskey::new(binding, device.clone(), credential.clone());
    let proposal = CandidateProposal {
        principal: trust.base.initial_principal.clone(),
        device: DeviceId::parse("00000000000000000000000020").unwrap(),
        credential: CredentialId::parse("00000000000000000000000021").unwrap(),
        public_key: candidate_key.verifying_key().to_bytes(),
        lifetime_secs: 300,
    };
    let (challenge, issued) = store.issue_registered_candidate(&proposal).unwrap();
    let proof = Proof::signed(challenge, &candidate_key, system_time().unwrap()).unwrap();
    let policy = registered_policy_commitment(
        &trust.base.organization,
        &trust.base.initial_principal,
        issued.checkpoint,
    );
    let message = existing_client_message(
        &trust.base.organization,
        &trust.base.initial_principal,
        device,
        credential,
        issued.checkpoint,
        policy,
        &proof.challenge,
    )
    .unwrap();
    let endorsement = ExistingClientEndorsement {
        device: device.clone(),
        credential: credential.clone(),
        signature: old_key.sign(&message).to_bytes(),
    };
    let (request, pending) = store
        .begin_registered_additional_assertion(&verifier, &passkey, issued, &proof, &endorsement)
        .unwrap();
    let before = store.checkpoint().unwrap();
    let assertion = response(&request, &passkey.binding, &candidate_key);
    let scoped = store
        .finish_registered_additional_assertion(&verifier, pending, &mut passkey, &assertion)
        .unwrap();
    assert_eq!(scoped.checkpoint(), before);
    assert_eq!(store.checkpoint().unwrap(), before);
    assert_eq!(fixture_state(&passkey.binding), (1, 1, false));
}

fn id(value: u64) -> String {
    format!("{value:026}")
}
fn device(value: u64) -> DeviceId {
    DeviceId::parse(&id(value)).unwrap()
}
fn credential(value: u64) -> CredentialId {
    CredentialId::parse(&id(value)).unwrap()
}

struct Fixture {
    store: AuthorityStoreRegistered,
    trust: FixtureRegisteredTrust,
    old_key: SigningKey,
    other_key: SigningKey,
    candidate_key: SigningKey,
    verifier: EnrollmentAssertionVerifier,
    passkey: FixtureRegisteredPasskey,
    proposal: CandidateProposal,
    root: TestRoot,
}

impl Fixture {
    fn new(granted: bool) -> Self {
        Self::with_owner(granted, 2)
    }

    fn with_owner(granted: bool, other_owner: u64) -> Self {
        let root = root();
        let (mut base, old_key, _) = authority_fixture();
        let other_key = SigningKey::from_bytes(&[0x35; 32]);
        base.known.push(
            KnownDevice::new(
                base.organization.clone(),
                PrincipalId::parse(&id(other_owner)).unwrap(),
                device(6),
                credential(7),
                other_key.verifying_key().to_bytes(),
            )
            .unwrap(),
        );
        let trust = FixtureRegisteredTrust {
            policy_admin: base.initial_principal.clone(),
            base,
        };
        let store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
        let (verifier, binding, candidate_key, _) = assertion_fixture();
        let passkey = FixtureRegisteredPasskey::new(binding, device(3), credential(4));
        let proposal = CandidateProposal {
            principal: trust.base.initial_principal.clone(),
            device: device(20),
            credential: credential(21),
            public_key: candidate_key.verifying_key().to_bytes(),
            lifetime_secs: 300,
        };
        let mut fixture = Self {
            store,
            trust,
            old_key,
            other_key,
            candidate_key,
            verifier,
            passkey,
            proposal,
            root,
        };
        if granted {
            fixture.set_grant(true);
        }
        fixture
    }

    fn apply(&mut self, action: V2Action) {
        let revision = self.store.checkpoint().unwrap().revision;
        let request = DeviceRequestV2 {
            organization: self.trust.base.organization.clone(),
            actor: self.trust.base.initial_principal.clone(),
            device: device(3),
            credential: credential(4),
            operation: OperationId::parse(&id(100 + revision)).unwrap(),
            base_revision: revision,
            action,
        };
        self.store
            .admit(&RegisteredRequest::Policy(SignedRequestV2 {
                signature: self.old_key.sign(&request.canonical_bytes()).to_bytes(),
                request,
            }))
            .unwrap();
    }

    fn set_grant(&mut self, enabled: bool) {
        self.apply(V2Action::SetOwnAdditionalDevice {
            principal: self.trust.base.initial_principal.clone(),
            enabled,
        });
    }

    fn revoke(&mut self, signer_other: bool, target_other: bool) {
        let revision = self.store.checkpoint().unwrap().revision;
        let request = RevocationRequest {
            organization: self.trust.base.organization.clone(),
            actor: self.trust.base.initial_principal.clone(),
            device: device(if signer_other { 6 } else { 3 }),
            credential: credential(if signer_other { 7 } else { 4 }),
            operation: OperationId::parse(&id(200 + revision)).unwrap(),
            base_revision: revision,
            target_device: device(if target_other { 6 } else { 3 }),
            target_credential: credential(if target_other { 7 } else { 4 }),
        };
        let key = if signer_other {
            &self.other_key
        } else {
            &self.old_key
        };
        self.store
            .admit(&RegisteredRequest::Revoke(SignedRevocation {
                signature: key.sign(&request.canonical_bytes()).to_bytes(),
                request,
            }))
            .unwrap();
    }

    fn endorse(
        &self,
        issued: &RegisteredIssuedCandidate,
        proof: &Proof,
    ) -> ExistingClientEndorsement {
        self.endorse_using(
            issued,
            proof,
            false,
            registered_policy_commitment(
                &self.trust.base.organization,
                &self.trust.base.initial_principal,
                issued.checkpoint,
            ),
        )
    }

    fn endorse_using(
        &self,
        issued: &RegisteredIssuedCandidate,
        proof: &Proof,
        other: bool,
        policy: [u8; 32],
    ) -> ExistingClientEndorsement {
        let device = device(if other { 6 } else { 3 });
        let credential = credential(if other { 7 } else { 4 });
        let message = existing_client_message(
            &self.trust.base.organization,
            &self.trust.base.initial_principal,
            &device,
            &credential,
            issued.checkpoint,
            policy,
            &proof.challenge,
        )
        .unwrap();
        let key = if other {
            &self.other_key
        } else {
            &self.old_key
        };
        ExistingClientEndorsement {
            device,
            credential,
            signature: key.sign(&message).to_bytes(),
        }
    }

    fn issued(&mut self) -> (RegisteredIssuedCandidate, Proof, ExistingClientEndorsement) {
        let (challenge, issued) = self
            .store
            .issue_registered_candidate(&self.proposal)
            .unwrap();
        let proof = Proof::signed(challenge, &self.candidate_key, system_time().unwrap()).unwrap();
        let endorsement = self.endorse(&issued, &proof);
        (issued, proof, endorsement)
    }

    fn begin(&mut self) -> (AssertionRequest, RegisteredAdditionalPending) {
        let (issued, proof, endorsement) = self.issued();
        self.store
            .begin_registered_additional_assertion(
                &self.verifier,
                &self.passkey,
                issued,
                &proof,
                &endorsement,
            )
            .unwrap()
    }

    fn finish(
        &mut self,
        request: &AssertionRequest,
        pending: RegisteredAdditionalPending,
    ) -> StoreResult<RegisteredScopedAssertion> {
        let assertion = response(request, &self.passkey.binding, &self.candidate_key);
        self.store.finish_registered_additional_assertion(
            &self.verifier,
            pending,
            &mut self.passkey,
            &assertion,
        )
    }
}

#[test]
fn registered_policy_commitment_matches_the_independent_fixture_vector() {
    let organization = OrganizationId::parse(&id(1)).unwrap();
    let owner = PrincipalId::parse(&id(2)).unwrap();
    let checkpoint = Checkpoint {
        revision: 7,
        digest: [0x42; 32],
    };
    let expected = registered_policy_commitment(&organization, &owner, checkpoint);
    assert_eq!(
        hex(&expected),
        "7b2c6c30beca48ea2dc1817be98f14a40c13f364aff1003d8acba6e8fdb1ed8a"
    );
    for actual in [
        registered_policy_commitment(&OrganizationId::parse(&id(3)).unwrap(), &owner, checkpoint),
        registered_policy_commitment(
            &organization,
            &PrincipalId::parse(&id(3)).unwrap(),
            checkpoint,
        ),
        registered_policy_commitment(
            &organization,
            &owner,
            Checkpoint {
                revision: 8,
                ..checkpoint
            },
        ),
        registered_policy_commitment(
            &organization,
            &owner,
            Checkpoint {
                digest: [0x43; 32],
                ..checkpoint
            },
        ),
    ] {
        assert_ne!(actual, expected);
    }
    let mut legacy = Sha256::new();
    legacy.update(b"vivac.fixture.additional-device-policy.v1\0");
    legacy.update(organization.as_str().as_bytes());
    legacy.update(1_u16.to_be_bytes());
    legacy.update(owner.as_str().as_bytes());
    assert_ne!(expected, <[u8; 32]>::from(legacy.finalize()));
}

#[test]
fn membership_and_policy_administration_do_not_replace_the_own_device_grant() {
    let mut fixture = Fixture::new(false);
    assert!(fixture
        .store
        .can_manage_enrollment_policy(
            &fixture.trust.base.organization,
            &fixture.trust.base.initial_principal
        )
        .unwrap());
    assert!(matches!(
        fixture.store.issue_registered_candidate(&fixture.proposal),
        Err(StoreError::Refused)
    ));
    fixture.set_grant(true);
    assert!(fixture
        .store
        .issue_registered_candidate(&fixture.proposal)
        .is_ok());
    fixture.set_grant(false);
    assert!(matches!(
        fixture.store.issue_registered_candidate(&fixture.proposal),
        Err(StoreError::Refused)
    ));
}

#[test]
fn candidate_identifiers_keys_and_owner_must_be_new_and_current() {
    let mut fixture = Fixture::new(true);
    for change in ["device", "credential", "key", "weak", "owner"] {
        let mut proposal = fixture.proposal.clone();
        match change {
            "device" => proposal.device = device(3),
            "credential" => proposal.credential = credential(4),
            "key" => proposal.public_key = fixture.old_key.verifying_key().to_bytes(),
            "weak" => proposal.public_key = [0; 32],
            "owner" => proposal.principal = PrincipalId::parse(&id(9)).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                fixture.store.issue_registered_candidate(&proposal),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
    }
}

#[test]
fn revoked_and_inactive_bindings_keep_their_identifiers_and_keys_reserved() {
    for inactive in [false, true] {
        let mut fixture = Fixture::with_owner(true, if inactive { 9 } else { 2 });
        if !inactive {
            fixture.revoke(false, true);
        }
        for change in ["device", "credential", "key"] {
            let mut proposal = fixture.proposal.clone();
            match change {
                "device" => proposal.device = device(6),
                "credential" => proposal.credential = credential(7),
                "key" => proposal.public_key = fixture.other_key.verifying_key().to_bytes(),
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    fixture.store.issue_registered_candidate(&proposal),
                    Err(StoreError::Refused)
                ),
                "{inactive}:{change}"
            );
        }
    }
}

#[test]
fn the_candidate_bound_counts_all_fixture_bindings_without_eviction() {
    let root = root();
    let (mut base, key, _) = authority_fixture();
    for index in 0..127_u64 {
        base.known.push(
            KnownDevice::new(
                base.organization.clone(),
                PrincipalId::parse(&id(9)).unwrap(),
                device(1000 + index),
                credential(2000 + index),
                SigningKey::from_bytes(&[index as u8; 32])
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap(),
        );
    }
    let trust = FixtureRegisteredTrust {
        policy_admin: base.initial_principal.clone(),
        base,
    };
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    grant(&mut store, &trust, &key);
    let key = SigningKey::from_bytes(&[0x99; 32]);
    let proposal = CandidateProposal {
        principal: trust.base.initial_principal.clone(),
        device: device(20),
        credential: credential(21),
        public_key: key.verifying_key().to_bytes(),
        lifetime_secs: 300,
    };
    let checkpoint = store.checkpoint().unwrap();
    assert!(matches!(
        store.issue_registered_candidate(&proposal),
        Err(StoreError::Refused)
    ));
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
}

fn failed_entropy(_: &mut [u8]) -> StoreResult<()> {
    Err(StoreError::Unavailable)
}
fn failed_clock() -> StoreResult<u64> {
    Err(StoreError::Unavailable)
}
fn overflow_clock() -> StoreResult<u64> {
    Ok(u64::MAX - 1)
}
fn known_operation_entropy(bytes: &mut [u8]) -> StoreResult<()> {
    bytes.fill(0);
    bytes[23] = 1;
    Ok(())
}
fn expired_clock() -> StoreResult<u64> {
    Ok(system_time()?.checked_sub(301).unwrap())
}
fn future_clock() -> StoreResult<u64> {
    Ok(system_time()? + 300)
}

#[test]
fn issuance_refuses_bad_lifetimes_clock_entropy_and_operation_collisions() {
    let mut fixture = Fixture::new(true);
    let checkpoint = fixture.store.checkpoint().unwrap();
    for lifetime in [0, 301] {
        let mut proposal = fixture.proposal.clone();
        proposal.lifetime_secs = lifetime;
        assert!(matches!(
            fixture.store.issue_registered_candidate(&proposal),
            Err(StoreError::Refused)
        ));
    }
    assert!(matches!(
        fixture.store.issue_registered_candidate_using(
            &fixture.proposal,
            failed_clock,
            system_entropy
        ),
        Err(StoreError::Unavailable)
    ));
    assert!(matches!(
        fixture.store.issue_registered_candidate_using(
            &fixture.proposal,
            system_time,
            failed_entropy
        ),
        Err(StoreError::Unavailable)
    ));
    assert!(matches!(
        fixture.store.issue_registered_candidate_using(
            &fixture.proposal,
            overflow_clock,
            system_entropy
        ),
        Err(StoreError::Refused)
    ));
    assert!(matches!(
        fixture.store.issue_registered_candidate_using(
            &fixture.proposal,
            system_time,
            known_operation_entropy
        ),
        Err(StoreError::Refused)
    ));
    assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn a_valid_replacement_proof_cannot_change_the_issued_candidate() {
    for change in ["device", "credential", "key", "nonce", "operation"] {
        let mut fixture = Fixture::new(true);
        let (issued, proof, _) = fixture.issued();
        let mut challenge = proof.challenge;
        let replacement = SigningKey::from_bytes(&[0x72; 32]);
        match change {
            "device" => challenge.device_id = id(22),
            "credential" => challenge.credential_id = id(23),
            "key" => challenge.public_key = hex(&replacement.verifying_key().to_bytes()),
            "nonce" => challenge.nonce = "44".repeat(32),
            "operation" => challenge.operation_id = id(24),
            _ => unreachable!(),
        }
        let key = if change == "key" {
            &replacement
        } else {
            &fixture.candidate_key
        };
        let proof = Proof::signed(challenge, key, system_time().unwrap()).unwrap();
        let endorsement = fixture.endorse(&issued, &proof);
        assert!(
            matches!(
                fixture.store.begin_registered_additional_assertion(
                    &fixture.verifier,
                    &fixture.passkey,
                    issued,
                    &proof,
                    &endorsement
                ),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
        assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
    }
}

#[test]
fn issuance_is_bound_to_the_physical_store_and_full_checkpoint() {
    for change in ["root", "checkpoint", "digest", "revision"] {
        let mut fixture = Fixture::new(true);
        let (mut issued, proof, endorsement) = fixture.issued();
        let mut other = Fixture::new(true);
        match change {
            "checkpoint" => fixture.set_grant(true),
            "digest" => issued.checkpoint.digest[0] ^= 1,
            "revision" => issued.checkpoint.revision += 1,
            "root" => {}
            _ => unreachable!(),
        }
        let store = if change == "root" {
            &mut other.store
        } else {
            &mut fixture.store
        };
        assert!(
            matches!(
                store.begin_registered_additional_assertion(
                    &fixture.verifier,
                    &fixture.passkey,
                    issued,
                    &proof,
                    &endorsement
                ),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
    }
}

#[test]
fn a_revoked_prior_client_cannot_begin_even_after_fresh_issuance() {
    let mut fixture = Fixture::new(true);
    fixture.revoke(true, false);
    let (issued, proof, endorsement) = fixture.issued();
    assert!(matches!(
        fixture.store.begin_registered_additional_assertion(
            &fixture.verifier,
            &fixture.passkey,
            issued,
            &proof,
            &endorsement
        ),
        Err(StoreError::Refused)
    ));
    assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
}

#[test]
fn begin_checks_own_identity_association_endorsement_signature_and_policy() {
    for change in [
        "organization",
        "principal",
        "association",
        "endorsement",
        "signature",
        "policy",
        "candidate_signer",
    ] {
        let mut fixture = Fixture::new(true);
        let (issued, proof, mut endorsement) = fixture.issued();
        match change {
            "organization" => {
                fixture.passkey.binding = fixture_binding_with_identity(
                    OrganizationId::parse(&id(9)).unwrap(),
                    fixture.trust.base.initial_principal.clone(),
                )
            }
            "principal" => {
                fixture.passkey.binding = fixture_binding_with_identity(
                    fixture.trust.base.organization.clone(),
                    PrincipalId::parse(&id(9)).unwrap(),
                )
            }
            "association" => fixture.passkey.device = device(6),
            "endorsement" => {
                endorsement = fixture.endorse_using(
                    &issued,
                    &proof,
                    true,
                    registered_policy_commitment(
                        &fixture.trust.base.organization,
                        &fixture.trust.base.initial_principal,
                        issued.checkpoint,
                    ),
                )
            }
            "signature" => endorsement.signature[0] ^= 1,
            "policy" => endorsement = fixture.endorse_using(&issued, &proof, false, [0x42; 32]),
            "candidate_signer" => {
                let policy = registered_policy_commitment(
                    &fixture.trust.base.organization,
                    &fixture.trust.base.initial_principal,
                    issued.checkpoint,
                );
                let message = existing_client_message(
                    &fixture.trust.base.organization,
                    &fixture.trust.base.initial_principal,
                    &endorsement.device,
                    &endorsement.credential,
                    issued.checkpoint,
                    policy,
                    &proof.challenge,
                )
                .unwrap();
                endorsement.signature = fixture.candidate_key.sign(&message).to_bytes();
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                fixture.store.begin_registered_additional_assertion(
                    &fixture.verifier,
                    &fixture.passkey,
                    issued,
                    &proof,
                    &endorsement
                ),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
        assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
    }
}

#[test]
fn expired_and_future_issued_proofs_refuse_without_changing_the_passkey() {
    for clock in [expired_clock as fn() -> StoreResult<u64>, future_clock] {
        let mut fixture = Fixture::new(true);
        let (challenge, issued) = fixture
            .store
            .issue_registered_candidate_using(&fixture.proposal, clock, system_entropy)
            .unwrap();
        let now = challenge.issued_at;
        let proof = Proof::signed(challenge, &fixture.candidate_key, now).unwrap();
        let endorsement = fixture.endorse(&issued, &proof);
        assert!(matches!(
            fixture.store.begin_registered_additional_assertion(
                &fixture.verifier,
                &fixture.passkey,
                issued,
                &proof,
                &endorsement
            ),
            Err(StoreError::Refused)
        ));
        assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
    }
}

#[test]
fn finish_refuses_revocation_or_a_removed_and_regranted_policy() {
    for revoke in [false, true] {
        let mut fixture = Fixture::new(true);
        let (request, pending) = fixture.begin();
        if revoke {
            fixture.revoke(true, false);
        } else {
            fixture.set_grant(false);
            fixture.set_grant(true);
        }
        assert!(matches!(
            fixture.finish(&request, pending),
            Err(StoreError::Refused)
        ));
        assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
    }
}

#[test]
fn another_handle_cannot_finish_a_check_after_the_registry_changes() {
    let mut fixture = Fixture::new(true);
    let checkpoint = fixture.store.checkpoint().unwrap();
    let mut other = AuthorityStoreRegistered::open_existing(
        fd(&fixture.root),
        fixture.trust.clone(),
        checkpoint,
    )
    .unwrap();
    let (request, pending) = fixture.begin();
    fixture.revoke(true, false);
    let assertion = response(&request, &fixture.passkey.binding, &fixture.candidate_key);
    assert!(matches!(
        other.finish_registered_additional_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.passkey,
            &assertion
        ),
        Err(StoreError::Refused)
    ));
    assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
}

#[test]
fn finish_refuses_another_assertion_context_or_a_different_passkey_instance() {
    for instance in [false, true] {
        let mut fixture = Fixture::new(true);
        let (request, pending) = fixture.begin();
        let assertion = if instance {
            let (_, binding, _, _) = assertion_fixture();
            fixture.passkey = FixtureRegisteredPasskey::new(binding, device(3), credential(4));
            response(&request, &fixture.passkey.binding, &fixture.candidate_key)
        } else {
            let (other, _) = fixture.begin();
            response(&other, &fixture.passkey.binding, &fixture.candidate_key)
        };
        assert!(matches!(
            fixture.store.finish_registered_additional_assertion(
                &fixture.verifier,
                pending,
                &mut fixture.passkey,
                &assertion
            ),
            Err(StoreError::Refused)
        ));
        assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
    }
}

#[test]
fn a_member_scope_assertion_cannot_replace_the_additional_scope() {
    let mut fixture = Fixture::new(true);
    let (_, mut pending) = fixture.begin();
    let (request, assertion) = fixture
        .verifier
        .start(&pending.proof, &fixture.passkey.binding)
        .unwrap();
    pending.assertion = assertion;
    assert!(matches!(
        fixture.finish(&request, pending),
        Err(StoreError::Refused)
    ));
    assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
}

#[test]
fn a_successful_check_invalidates_concurrent_pending_assertions_for_the_binding() {
    let mut fixture = Fixture::new(true);
    let (first, first_pending) = fixture.begin();
    let (second, second_pending) = fixture.begin();
    fixture.finish(&first, first_pending).unwrap();
    assert!(matches!(
        fixture.finish(&second, second_pending),
        Err(StoreError::Refused)
    ));
    assert_eq!(fixture_state(&fixture.passkey.binding), (1, 1, false));
}

fn fault_before(point: FaultPoint) -> bool {
    point == FaultPoint::BeforeCommit
}
fn fault_after(point: FaultPoint) -> bool {
    point == FaultPoint::AfterCommit
}

#[test]
fn failed_or_uncertain_finish_does_not_publish_the_prepared_passkey_update() {
    for fault in [fault_before as fn(FaultPoint) -> bool, fault_after] {
        let mut fixture = Fixture::new(true);
        let checkpoint = fixture.store.checkpoint().unwrap();
        let (request, pending) = fixture.begin();
        fixture.store.set_fault_hook(Some(fault));
        let result = fixture.finish(&request, pending);
        let expected = if fault(FaultPoint::AfterCommit) {
            StoreError::CommitUnknown
        } else {
            StoreError::Unpublished
        };
        assert!(matches!(result, Err(error) if error == expected));
        assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
        if fault(FaultPoint::AfterCommit) {
            assert_eq!(fixture.store.checkpoint(), Err(StoreError::CommitUnknown));
        } else {
            fixture.store.set_fault_hook(None);
            assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
        }
        let other = AuthorityStoreRegistered::open_existing(
            fd(&fixture.root),
            fixture.trust.clone(),
            checkpoint,
        )
        .unwrap();
        drop(other);
    }
}

#[test]
fn issuance_and_begin_do_not_return_tokens_after_transaction_failure() {
    for stage in ["issue", "begin"] {
        for fault in [fault_before as fn(FaultPoint) -> bool, fault_after] {
            let mut fixture = Fixture::new(true);
            let issued = if stage == "begin" {
                Some(fixture.issued())
            } else {
                None
            };
            fixture.store.set_fault_hook(Some(fault));
            let expected = if fault(FaultPoint::AfterCommit) {
                StoreError::CommitUnknown
            } else {
                StoreError::Unpublished
            };
            if let Some((issued, proof, endorsement)) = issued {
                assert!(
                    matches!(fixture.store.begin_registered_additional_assertion(&fixture.verifier, &fixture.passkey, issued, &proof, &endorsement), Err(error) if error == expected)
                );
            } else {
                assert!(
                    matches!(fixture.store.issue_registered_candidate(&fixture.proposal), Err(error) if error == expected)
                );
            }
            assert_eq!(fixture_state(&fixture.passkey.binding), (0, 0, false));
        }
    }
}

#[test]
fn checks_do_not_write_history_admit_bindings_or_persist_passkey_updates() {
    let mut fixture = Fixture::new(true);
    let checkpoint = fixture.store.checkpoint().unwrap();
    let database = fixture.root.path().join("authority.sqlite3");
    let before = fs::read(&database).unwrap();
    let (request, pending) = fixture.begin();
    fixture.finish(&request, pending).unwrap();
    assert_eq!(fs::read(&database).unwrap(), before);
    assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
    assert!(!fixture
        .store
        .binding_is_active(
            &fixture.trust.base.organization,
            &fixture.trust.base.initial_principal,
            &fixture.proposal.device,
            &fixture.proposal.credential
        )
        .unwrap());
    fixture.store = AuthorityStoreRegistered::open_existing(
        fd(&fixture.root),
        fixture.trust.clone(),
        checkpoint,
    )
    .unwrap();
    assert!(!fixture
        .store
        .binding_is_active(
            &fixture.trust.base.organization,
            &fixture.trust.base.initial_principal,
            &fixture.proposal.device,
            &fixture.proposal.credential
        )
        .unwrap());
    let (_, reconstructed, _, _) = assertion_fixture();
    assert_eq!(fixture_state(&reconstructed), (0, 0, false));
    assert_eq!(fixture_state(&fixture.passkey.binding), (1, 1, false));
}
