use super::super::super::super::tests::{fd, fixture as authority_fixture, root};
use super::super::super::super::tests::{sign, TestRoot};
use super::super::super::super::FixtureTrust;
use super::super::super::CandidateProposal;
use super::*;
use crate::approval::tests::fixture_binding_with_identity;
use crate::approval::tests::{fixture as assertion_fixture, fixture_state, response};
use crate::enrollment_proof::Proof;
use crate::team_authority::signed::{DeviceRequest, KnownDevice};
use crate::team_authority::{Action, OrganizationCapability};
use crate::team_authority::{CredentialId, DeviceId};
use base64ct::{Base64UrlUnpadded, Encoding};
use ed25519_dalek::{Signer, SigningKey};

#[test]
fn additional_device_challenge_commits_to_its_endorsed_context() {
    let mut fixture = Fixture::new(Spec::default());
    let checkpoint = fixture.store.checkpoint().unwrap();
    let endorsement = fixture.endorsement();
    let message = existing_client_message(
        fixture.binding.organization(),
        fixture.binding.principal(),
        &endorsement.device,
        &endorsement.credential,
        checkpoint,
        fixture.policy.fingerprint(),
        &fixture.proof.challenge,
    )
    .unwrap();
    assert_eq!(message.len(), 466);
    let mut context = Sha256::new();
    context.update(b"vivac.device.additional-context.v1\0");
    context.update(message);
    context.update(endorsement.signature);
    let context: [u8; 32] = context.finalize().into();
    assert_eq!(
        additional_context_digest(
            fixture.binding.organization(),
            fixture.binding.principal(),
            checkpoint,
            fixture.policy.fingerprint(),
            &fixture.proof.challenge,
            &endorsement,
        )
        .unwrap(),
        context
    );
    let (request, _) = fixture.begin();
    let mut enrollment = Sha256::new();
    enrollment.update(b"vivac.device.enrollment-assertion.v1\0");
    enrollment.update(fixture.proof.challenge.canonical_bytes().unwrap());
    enrollment.update(fixture.binding.organization().as_str().as_bytes());
    enrollment.update(fixture.binding.principal().as_str().as_bytes());
    for field in [
        &[0x21; 32][..],
        b"vivac.test".as_slice(),
        b"https://vivac.test".as_slice(),
    ] {
        enrollment.update(u16::try_from(field.len()).unwrap().to_be_bytes());
        enrollment.update(field);
    }
    enrollment.update(request.context.nonce);
    enrollment.update(request.context.issued_at.to_be_bytes());
    enrollment.update(request.context.expires_at.to_be_bytes());
    let mut additional = Sha256::new();
    additional.update(b"vivac.device.additional-assertion.v1\0");
    additional.update(enrollment.finalize());
    additional.update(context);
    assert_eq!(
        request.options.challenge,
        Base64UrlUnpadded::encode_string(&additional.finalize())
    );
}

#[test]
fn an_explicitly_granted_owner_can_check_its_new_additional_device() {
    let root = root();
    let (trust, old_key, old_request) = authority_fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let (verifier, mut binding, key, _) = assertion_fixture();
    let policy = FixtureAdditionalDevicePolicy::new(
        trust.organization.clone(),
        vec![trust.initial_principal.clone()],
    )
    .unwrap();
    let proposal = CandidateProposal {
        principal: trust.initial_principal.clone(),
        device: DeviceId::parse("00000000000000000000000004").unwrap(),
        credential: CredentialId::parse("00000000000000000000000005").unwrap(),
        public_key: key.verifying_key().to_bytes(),
        lifetime_secs: 300,
    };
    let challenge = store
        .issue_candidate(
            &OperationId::parse("00000000000000000000000012").unwrap(),
            &proposal,
        )
        .unwrap();
    let operation = OperationId::parse(&challenge.operation_id).unwrap();
    let proof = Proof::signed(
        challenge,
        &key,
        crate::enrollment_proof::current_time().unwrap(),
    )
    .unwrap();
    store
        .observe_candidate(&serde_json::to_vec(&proof).unwrap())
        .unwrap();
    let checkpoint = store.checkpoint().unwrap();
    let endorsement = sign_endorsement(
        checkpoint,
        &policy,
        &proof,
        &old_request.device,
        &old_request.credential,
        &old_key,
    );
    let (request, pending) = store
        .begin_additional_device_assertion(&verifier, &binding, &operation, &policy, &endorsement)
        .unwrap();
    let assertion = response(&request, &binding, &key);
    let checked = store
        .finish_additional_device_assertion(&verifier, pending, &mut binding, &assertion, &policy)
        .unwrap();
    assert_eq!(checked.checkpoint(), checkpoint);
    assert_eq!(fixture_state(&binding), (1, 1, false));
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
}

#[derive(Clone, Copy)]
enum Reuse {
    None,
    OwnerDevice,
    OwnerCredential,
    OwnerKey,
    OtherDevice,
    OtherCredential,
    OtherKey,
}

struct Spec {
    subject: u64,
    approver: u64,
    known_subject: bool,
    active_subject: bool,
    privileged_subject: bool,
    reuse: Reuse,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            subject: 2,
            approver: 2,
            known_subject: true,
            active_subject: true,
            privileged_subject: false,
            reuse: Reuse::None,
        }
    }
}

struct Fixture {
    store: AuthorityStore,
    _root: TestRoot,
    trust: FixtureTrust,
    authority_key: SigningKey,
    authority_request: DeviceRequest,
    verifier: EnrollmentAssertionVerifier,
    binding: TrustedPasskeyBinding,
    assertion_key: SigningKey,
    owner_key: SigningKey,
    owner_device: DeviceId,
    owner_credential: CredentialId,
    operation: OperationId,
    proof: Proof,
    policy: FixtureAdditionalDevicePolicy,
}

fn principal(number: u64) -> PrincipalId {
    PrincipalId::parse(&format!("{number:026}")).unwrap()
}

fn capabilities() -> BTreeSet<OrganizationCapability> {
    [
        OrganizationCapability::ManageMembers,
        OrganizationCapability::ManageProjects,
        OrganizationCapability::GrantProjectAccess,
        OrganizationCapability::ManageOrganizationCapabilities,
    ]
    .into_iter()
    .collect()
}

fn apply_change(
    store: &mut AuthorityStore,
    actor: &DeviceRequest,
    key: &SigningKey,
    action: Action,
) {
    let mut request = actor.clone();
    request.change.base_revision = store.checkpoint().unwrap().revision;
    request.change.operation =
        OperationId::parse(&format!("{:026}", 100 + request.change.base_revision)).unwrap();
    request.change.action = action;
    store.admit(&sign(request, key)).unwrap();
}

fn sign_endorsement(
    checkpoint: Checkpoint,
    policy: &FixtureAdditionalDevicePolicy,
    proof: &Proof,
    device: &DeviceId,
    credential: &CredentialId,
    key: &SigningKey,
) -> ExistingClientEndorsement {
    let message = existing_client_message(
        &OrganizationId::parse(&proof.challenge.organization_id).unwrap(),
        &PrincipalId::parse(&proof.challenge.principal_id).unwrap(),
        device,
        credential,
        checkpoint,
        policy.fingerprint(),
        &proof.challenge,
    )
    .unwrap();
    ExistingClientEndorsement {
        device: device.clone(),
        credential: credential.clone(),
        signature: key.sign(&message).to_bytes(),
    }
}

impl Fixture {
    fn new(spec: Spec) -> Self {
        let root = root();
        let (mut trust, authority_key, authority_request) = authority_fixture();
        let owner_key = if spec.subject == 2 {
            SigningKey::from_bytes(&authority_key.to_bytes())
        } else {
            SigningKey::from_bytes(&[0x53; 32])
        };
        let other_key = SigningKey::from_bytes(&[0x52; 32]);
        let owner_device = if spec.subject == 2 { 3 } else { 10 };
        let owner_credential = if spec.subject == 2 { 4 } else { 11 };
        if spec.subject == 2 && !spec.known_subject {
            trust.known.clear();
        } else if spec.subject != 2 && spec.known_subject {
            trust.known.push(
                KnownDevice::new(
                    trust.organization.clone(),
                    principal(spec.subject),
                    DeviceId::parse(&format!("{owner_device:026}")).unwrap(),
                    CredentialId::parse(&format!("{owner_credential:026}")).unwrap(),
                    owner_key.verifying_key().to_bytes(),
                )
                .unwrap(),
            );
        }
        trust.known.push(
            KnownDevice::new(
                trust.organization.clone(),
                principal(7),
                DeviceId::parse("00000000000000000000000008").unwrap(),
                CredentialId::parse("00000000000000000000000009").unwrap(),
                other_key.verifying_key().to_bytes(),
            )
            .unwrap(),
        );
        let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
        if spec.subject != 2 && spec.active_subject {
            apply_change(
                &mut store,
                &authority_request,
                &authority_key,
                Action::AddPrincipal {
                    principal: principal(spec.subject),
                },
            );
            if spec.privileged_subject {
                apply_change(
                    &mut store,
                    &authority_request,
                    &authority_key,
                    Action::SetOrganizationCapabilities {
                        principal: principal(spec.subject),
                        capabilities: capabilities(),
                    },
                );
            }
        }
        let (verifier, _, assertion_key, _) = assertion_fixture();
        let binding =
            fixture_binding_with_identity(trust.organization.clone(), principal(spec.approver));
        let candidate_key = match spec.reuse {
            Reuse::OwnerKey => SigningKey::from_bytes(&owner_key.to_bytes()),
            Reuse::OtherKey => other_key,
            _ => SigningKey::from_bytes(&assertion_key.to_bytes()),
        };
        let device = match spec.reuse {
            Reuse::OwnerDevice => owner_device,
            Reuse::OtherDevice => 8,
            _ => 20,
        };
        let credential = match spec.reuse {
            Reuse::OwnerCredential => owner_credential,
            Reuse::OtherCredential => 9,
            _ => 21,
        };
        let proposal = CandidateProposal {
            principal: principal(spec.subject),
            device: DeviceId::parse(&format!("{device:026}")).unwrap(),
            credential: CredentialId::parse(&format!("{credential:026}")).unwrap(),
            public_key: candidate_key.verifying_key().to_bytes(),
            lifetime_secs: 300,
        };
        let challenge = store
            .issue_candidate(
                &OperationId::parse("00000000000000000000000030").unwrap(),
                &proposal,
            )
            .unwrap();
        let operation = OperationId::parse(&challenge.operation_id).unwrap();
        let proof = Proof::signed(
            challenge,
            &candidate_key,
            crate::enrollment_proof::current_time().unwrap(),
        )
        .unwrap();
        store
            .observe_candidate(&serde_json::to_vec(&proof).unwrap())
            .unwrap();
        let policy = FixtureAdditionalDevicePolicy::new(
            trust.organization.clone(),
            vec![principal(spec.approver)],
        )
        .unwrap();
        Self {
            store,
            _root: root,
            trust,
            authority_key,
            authority_request,
            verifier,
            binding,
            assertion_key,
            owner_key,
            owner_device: DeviceId::parse(&format!("{owner_device:026}")).unwrap(),
            owner_credential: CredentialId::parse(&format!("{owner_credential:026}")).unwrap(),
            operation,
            proof,
            policy,
        }
    }

    fn begin(&mut self) -> (AssertionRequest, AdditionalPending) {
        self.try_begin().unwrap()
    }

    fn endorsement(&mut self) -> ExistingClientEndorsement {
        sign_endorsement(
            self.store.checkpoint().unwrap(),
            &self.policy,
            &self.proof,
            &self.owner_device,
            &self.owner_credential,
            &self.owner_key,
        )
    }

    fn try_begin(&mut self) -> StoreResult<(AssertionRequest, AdditionalPending)> {
        let endorsement = self.endorsement();
        self.store.begin_additional_device_assertion(
            &self.verifier,
            &self.binding,
            &self.operation,
            &self.policy,
            &endorsement,
        )
    }

    fn try_begin_with_policy(
        &mut self,
        policy: &FixtureAdditionalDevicePolicy,
    ) -> StoreResult<(AssertionRequest, AdditionalPending)> {
        let endorsement = sign_endorsement(
            self.store.checkpoint().unwrap(),
            policy,
            &self.proof,
            &self.owner_device,
            &self.owner_credential,
            &self.owner_key,
        );
        self.store.begin_additional_device_assertion(
            &self.verifier,
            &self.binding,
            &self.operation,
            policy,
            &endorsement,
        )
    }

    fn unchanged(&self) {
        assert_eq!(fixture_state(&self.binding), (0, 0, false));
    }
}

#[test]
fn an_ordinary_active_owner_needs_no_member_administration_to_use_its_explicit_grant() {
    let mut fixture = Fixture::new(Spec {
        subject: 3,
        approver: 3,
        ..Spec::default()
    });
    assert!(!fixture
        .store
        .replay()
        .unwrap()
        .0
        .can_manage_members(fixture.binding.organization(), fixture.binding.principal()));
    let checkpoint = fixture.store.checkpoint().unwrap();
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.assertion_key);
    let checked = fixture
        .store
        .finish_additional_device_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion,
            &fixture.policy,
        )
        .unwrap();
    assert_eq!(checked.checkpoint(), checkpoint);
    assert_eq!(fixture_state(&fixture.binding), (1, 1, false));
    assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn all_administrative_capabilities_cannot_substitute_for_another_privileged_owner() {
    let mut fixture = Fixture::new(Spec {
        subject: 3,
        approver: 2,
        privileged_subject: true,
        ..Spec::default()
    });
    let authority = fixture.store.replay().unwrap().0;
    assert!(
        authority.can_manage_members(fixture.binding.organization(), fixture.binding.principal())
    );
    assert!(authority.can_manage_members(&fixture.trust.organization, &principal(3)));
    fixture.policy = FixtureAdditionalDevicePolicy::new(
        fixture.trust.organization.clone(),
        vec![principal(2), principal(3)],
    )
    .unwrap();
    assert!(matches!(fixture.try_begin(), Err(StoreError::Refused)));
    fixture.unchanged();
}

#[test]
fn membership_or_administration_does_not_create_an_implicit_additional_device_grant() {
    for granted in [Vec::new(), vec![principal(3)]] {
        let mut fixture = Fixture::new(Spec::default());
        let policy =
            FixtureAdditionalDevicePolicy::new(fixture.trust.organization.clone(), granted)
                .unwrap();
        assert!(matches!(
            fixture.try_begin_with_policy(&policy),
            Err(StoreError::Refused)
        ));
        fixture.unchanged();
    }
}

#[test]
fn a_passkey_fixture_without_a_previously_known_client_cannot_bootstrap_or_recover() {
    for spec in [
        Spec {
            known_subject: false,
            ..Spec::default()
        },
        Spec {
            subject: 3,
            approver: 3,
            known_subject: false,
            ..Spec::default()
        },
    ] {
        let mut fixture = Fixture::new(spec);
        assert!(fixture
            .store
            .replay()
            .unwrap()
            .0
            .is_active_principal(fixture.binding.organization(), fixture.binding.principal()));
        assert!(matches!(fixture.try_begin(), Err(StoreError::Refused)));
        fixture.unchanged();
    }
}

#[test]
fn explicit_grants_do_not_activate_unknown_principals() {
    let mut fixture = Fixture::new(Spec {
        subject: 3,
        approver: 3,
        active_subject: false,
        ..Spec::default()
    });
    assert!(matches!(fixture.try_begin(), Err(StoreError::Refused)));
    fixture.unchanged();
}

#[test]
fn additional_device_policy_never_reuses_an_existing_device_credential_or_key() {
    for reuse in [
        Reuse::OwnerDevice,
        Reuse::OwnerCredential,
        Reuse::OwnerKey,
        Reuse::OtherDevice,
        Reuse::OtherCredential,
        Reuse::OtherKey,
    ] {
        let mut fixture = Fixture::new(Spec {
            reuse,
            ..Spec::default()
        });
        assert!(matches!(fixture.try_begin(), Err(StoreError::Refused)));
        fixture.unchanged();
    }
}

#[test]
fn fixture_catalog_is_bounded_duplicate_free_and_order_independent() {
    let organization = OrganizationId::parse("00000000000000000000000001").unwrap();
    let first =
        FixtureAdditionalDevicePolicy::new(organization.clone(), vec![principal(2), principal(3)])
            .unwrap();
    let second =
        FixtureAdditionalDevicePolicy::new(organization.clone(), vec![principal(3), principal(2)])
            .unwrap();
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert!(matches!(
        FixtureAdditionalDevicePolicy::new(organization.clone(), vec![principal(2), principal(2)]),
        Err(StoreError::Refused)
    ));
    FixtureAdditionalDevicePolicy::new(organization.clone(), (0..128).map(principal).collect())
        .unwrap();
    assert!(matches!(
        FixtureAdditionalDevicePolicy::new(organization.clone(), (0..129).map(principal).collect()),
        Err(StoreError::Refused)
    ));
    let other = FixtureAdditionalDevicePolicy::new(
        OrganizationId::parse("00000000000000000000000008").unwrap(),
        vec![principal(2), principal(3)],
    )
    .unwrap();
    assert_ne!(first.fingerprint(), other.fingerprint());
}

#[test]
fn a_changed_policy_invalidates_the_ceremony_even_when_the_owner_still_has_a_grant() {
    let mut fixture = Fixture::new(Spec::default());
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.assertion_key);
    let changed = FixtureAdditionalDevicePolicy::new(
        fixture.trust.organization.clone(),
        vec![principal(2), principal(3)],
    )
    .unwrap();
    assert!(matches!(
        fixture.store.finish_additional_device_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion,
            &changed
        ),
        Err(StoreError::Refused)
    ));
    fixture.unchanged();
}

#[test]
fn the_same_catalog_content_can_finish_when_its_input_order_changes() {
    let mut fixture = Fixture::new(Spec::default());
    fixture.policy = FixtureAdditionalDevicePolicy::new(
        fixture.trust.organization.clone(),
        vec![principal(3), principal(2)],
    )
    .unwrap();
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.assertion_key);
    let same = FixtureAdditionalDevicePolicy::new(
        fixture.trust.organization.clone(),
        vec![principal(2), principal(3)],
    )
    .unwrap();
    fixture
        .store
        .finish_additional_device_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion,
            &same,
        )
        .unwrap();
    assert_eq!(fixture_state(&fixture.binding), (1, 1, false));
}

#[test]
fn another_organization_cannot_supply_the_additional_device_policy() {
    let mut fixture = Fixture::new(Spec::default());
    let policy = FixtureAdditionalDevicePolicy::new(
        OrganizationId::parse("00000000000000000000000008").unwrap(),
        vec![principal(2)],
    )
    .unwrap();
    assert!(matches!(
        fixture.try_begin_with_policy(&policy),
        Err(StoreError::Refused)
    ));
    fixture.unchanged();
}

#[test]
fn revocation_or_an_unrelated_authority_change_refuses_the_open_additional_ceremony() {
    for revoke in [true, false] {
        let mut fixture = Fixture::new(Spec {
            subject: 3,
            approver: 3,
            ..Spec::default()
        });
        let (request, pending) = fixture.begin();
        let assertion = response(&request, &fixture.binding, &fixture.assertion_key);
        let action = if revoke {
            Action::RevokePrincipal {
                principal: principal(3),
            }
        } else {
            Action::CreateProject {
                project: crate::team_authority::ProjectId::parse("00000000000000000000000040")
                    .unwrap(),
            }
        };
        apply_change(
            &mut fixture.store,
            &fixture.authority_request,
            &fixture.authority_key,
            action,
        );
        assert!(matches!(
            fixture.store.finish_additional_device_assertion(
                &fixture.verifier,
                pending,
                &mut fixture.binding,
                &assertion,
                &fixture.policy
            ),
            Err(StoreError::Refused)
        ));
        fixture.unchanged();
    }
}

#[test]
fn transaction_failures_do_not_publish_an_additional_device_credential_update() {
    use super::super::FaultPoint;
    for begin in [true, false] {
        for point in [FaultPoint::BeforeCommit, FaultPoint::AfterCommit] {
            let mut fixture = Fixture::new(Spec::default());
            let checkpoint = fixture.store.checkpoint().unwrap();
            let setup = if begin { None } else { Some(fixture.begin()) };
            fixture.store.set_fault_hook(Some(match point {
                FaultPoint::BeforeCommit => |seen| seen == FaultPoint::BeforeCommit,
                FaultPoint::AfterCommit => |seen| seen == FaultPoint::AfterCommit,
                _ => unreachable!(),
            }));
            let error = if let Some((request, pending)) = setup {
                let assertion = response(&request, &fixture.binding, &fixture.assertion_key);
                fixture
                    .store
                    .finish_additional_device_assertion(
                        &fixture.verifier,
                        pending,
                        &mut fixture.binding,
                        &assertion,
                        &fixture.policy,
                    )
                    .err()
            } else {
                fixture.try_begin().err()
            };
            assert_eq!(
                error,
                Some(if point == FaultPoint::BeforeCommit {
                    StoreError::Unpublished
                } else {
                    StoreError::CommitUnknown
                })
            );
            fixture.unchanged();
            if point == FaultPoint::BeforeCommit {
                assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
            } else {
                assert_eq!(fixture.store.checkpoint(), Err(StoreError::CommitUnknown));
            }
        }
    }
}

#[test]
fn an_existing_client_endorsement_requires_the_exact_owner_key_and_signature_domain() {
    for invalid in 0..7 {
        let mut fixture = Fixture::new(Spec::default());
        let checkpoint = fixture.store.checkpoint().unwrap();
        let mut endorsement = fixture.endorsement();
        match invalid {
            0 => endorsement.signature = [0; 64],
            1 => endorsement.device = DeviceId::parse("00000000000000000000000088").unwrap(),
            2 => {
                endorsement.credential = CredentialId::parse("00000000000000000000000089").unwrap()
            }
            3 => {
                let key = SigningKey::from_bytes(&[0x52; 32]);
                endorsement = sign_endorsement(
                    checkpoint,
                    &fixture.policy,
                    &fixture.proof,
                    &fixture.owner_device,
                    &fixture.owner_credential,
                    &key,
                );
            }
            4 => {
                endorsement.device = DeviceId::parse("00000000000000000000000008").unwrap();
                endorsement.credential = CredentialId::parse("00000000000000000000000009").unwrap();
                let key = SigningKey::from_bytes(&[0x52; 32]);
                endorsement = sign_endorsement(
                    checkpoint,
                    &fixture.policy,
                    &fixture.proof,
                    &endorsement.device,
                    &endorsement.credential,
                    &key,
                );
            }
            5 => endorsement.signature = decode_hex::<64>(&fixture.proof.signature).unwrap(),
            6 => {
                endorsement.signature = fixture
                    .owner_key
                    .sign(&fixture.authority_request.canonical_bytes())
                    .to_bytes()
            }
            _ => unreachable!(),
        }
        assert!(matches!(
            fixture.store.begin_additional_device_assertion(
                &fixture.verifier,
                &fixture.binding,
                &fixture.operation,
                &fixture.policy,
                &endorsement
            ),
            Err(StoreError::Refused)
        ));
        fixture.unchanged();
        assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
    }
}

#[test]
fn an_existing_client_endorsement_cannot_be_reused_for_changed_candidate_checkpoint_or_policy() {
    for changed in 0..4 {
        let mut fixture = Fixture::new(Spec::default());
        let checkpoint = fixture.store.checkpoint().unwrap();
        let mut signed_proof = fixture.proof.clone();
        let mut signed_checkpoint = checkpoint;
        match changed {
            0 => signed_proof.challenge.nonce = "ab".repeat(32),
            1 => signed_checkpoint.digest[0] ^= 1,
            2 => {
                signed_checkpoint.revision += 1;
                signed_proof.challenge.base_revision += 1;
            }
            3 => {}
            _ => unreachable!(),
        }
        let other_policy = FixtureAdditionalDevicePolicy::new(
            fixture.trust.organization.clone(),
            vec![principal(2), principal(3)],
        )
        .unwrap();
        let signed_policy = if changed == 3 {
            &other_policy
        } else {
            &fixture.policy
        };
        let endorsement = sign_endorsement(
            signed_checkpoint,
            signed_policy,
            &signed_proof,
            &fixture.owner_device,
            &fixture.owner_credential,
            &fixture.owner_key,
        );
        assert!(matches!(
            fixture.store.begin_additional_device_assertion(
                &fixture.verifier,
                &fixture.binding,
                &fixture.operation,
                &fixture.policy,
                &endorsement
            ),
            Err(StoreError::Refused)
        ));
        fixture.unchanged();
        assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
    }
}

#[test]
fn finish_rechecks_the_captured_existing_client_endorsement_before_preparing_the_passkey_update() {
    let mut fixture = Fixture::new(Spec::default());
    let checkpoint = fixture.store.checkpoint().unwrap();
    let (request, mut pending) = fixture.begin();
    pending.endorsement.signature = [0; 64];
    let assertion = response(&request, &fixture.binding, &fixture.assertion_key);
    assert!(matches!(
        fixture.store.finish_additional_device_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion,
            &fixture.policy
        ),
        Err(StoreError::Refused)
    ));
    fixture.unchanged();
    assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn the_existing_client_message_matches_an_independent_canonical_vector() {
    let organization = OrganizationId::parse(&format!("{:026}", 1)).unwrap();
    let actor = principal(3);
    let device = DeviceId::parse(&format!("{:026}", 8)).unwrap();
    let credential = CredentialId::parse(&format!("{:026}", 9)).unwrap();
    let checkpoint = Checkpoint {
        revision: 7,
        digest: [0xcd; 32],
    };
    let challenge = Challenge {
        version: 1,
        organization_id: organization.as_str().into(),
        principal_id: actor.as_str().into(),
        device_id: format!("{:026}", 4),
        credential_id: format!("{:026}", 5),
        public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
        operation_id: format!("{:026}", 6),
        base_revision: 7,
        nonce: "23".repeat(32),
        issued_at: 1000,
        expires_at: 1300,
    };
    let message = existing_client_message(
        &organization,
        &actor,
        &device,
        &credential,
        checkpoint,
        [0xef; 32],
        &challenge,
    )
    .unwrap();
    assert_eq!(message.len(), 466);
    // This signature is an encoding fixture, not a valid authorization.
    let context_digest = additional_context_digest(
        &organization,
        &actor,
        checkpoint,
        [0xef; 32],
        &challenge,
        &ExistingClientEndorsement {
            device: device.clone(),
            credential: credential.clone(),
            signature: [0x42; 64],
        },
    )
    .unwrap();
    assert_eq!(
        context_digest,
        decode_hex::<32>("599e414eecf5600ea23052dc7e33190a930986b6d00de9932f30814a254fd687")
            .unwrap()
    );
    let assertion_digest = crate::approval::additional_commitment(
        &challenge,
        &organization,
        &principal(2),
        b"fixture-credential",
        "vivac.test",
        "https://vivac.test",
        &crate::approval::AssertionContext {
            nonce: [0xab; 32],
            issued_at: 1100,
            expires_at: 1300,
        },
        context_digest,
    )
    .unwrap();
    assert_eq!(
        assertion_digest,
        decode_hex::<32>("4d189097e03021ea6ca6f1f75d5c787ab341fddf37434c307e23badc8f7ed46c")
            .unwrap()
    );
    let digest: [u8; 32] = Sha256::digest(&message).into();
    assert_eq!(
        digest,
        decode_hex::<32>("35173169d12c429d982984d5e4000c8474e1ec882eded061060fc4a2f554d080")
            .unwrap()
    );
    let foreign = OrganizationId::parse(&format!("{:026}", 2)).unwrap();
    assert!(matches!(
        existing_client_message(
            &foreign,
            &actor,
            &device,
            &credential,
            checkpoint,
            [0xef; 32],
            &challenge
        ),
        Err(StoreError::Refused)
    ));
    assert!(matches!(
        existing_client_message(
            &organization,
            &principal(2),
            &device,
            &credential,
            checkpoint,
            [0xef; 32],
            &challenge
        ),
        Err(StoreError::Refused)
    ));
    assert!(matches!(
        existing_client_message(
            &organization,
            &actor,
            &device,
            &credential,
            Checkpoint {
                revision: 8,
                ..checkpoint
            },
            [0xef; 32],
            &challenge
        ),
        Err(StoreError::Refused)
    ));
}
