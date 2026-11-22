use super::super::super::tests::{fd, fixture as authority_fixture, root, sign, TestRoot};
use super::super::super::FixtureTrust;
use super::super::CandidateProposal;
use super::*;
use crate::approval::tests::{
    fixture as assertion_fixture, fixture_binding_with_identity, fixture_state, response,
};
use crate::enrollment_proof::Proof;
use crate::team_authority::signed::{DeviceRequest, KnownDevice};
use crate::team_authority::{
    Action, CredentialId, DeviceId, OrganizationCapability, OrganizationId, PrincipalId, ProjectId,
};
use ed25519_dalek::SigningKey;

struct Fixture {
    store: AuthorityStore,
    root: TestRoot,
    trust: FixtureTrust,
    authority_key: SigningKey,
    authority_request: DeviceRequest,
    verifier: EnrollmentAssertionVerifier,
    binding: TrustedPasskeyBinding,
    key: SigningKey,
    operation: OperationId,
    proof: Proof,
}

impl Fixture {
    fn new(observed: bool) -> Self {
        Self::build(observed, false)
    }

    fn build(observed: bool, second_admin: bool) -> Self {
        let root = root();
        let (mut trust, authority_key, authority_request) = authority_fixture();
        if second_admin {
            trust.known.push(
                KnownDevice::new(
                    trust.organization.clone(),
                    PrincipalId::parse("00000000000000000000000003").unwrap(),
                    DeviceId::parse("00000000000000000000000008").unwrap(),
                    CredentialId::parse("00000000000000000000000009").unwrap(),
                    authority_key.verifying_key().to_bytes(),
                )
                .unwrap(),
            );
        }
        let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
        if second_admin {
            apply_change(
                &mut store,
                &authority_request,
                &authority_key,
                Action::AddPrincipal {
                    principal: PrincipalId::parse("00000000000000000000000003").unwrap(),
                },
            );
            apply_change(
                &mut store,
                &authority_request,
                &authority_key,
                Action::SetOrganizationCapabilities {
                    principal: PrincipalId::parse("00000000000000000000000003").unwrap(),
                    capabilities: all_capabilities(),
                },
            );
        }
        let (verifier, binding, key, _) = assertion_fixture();
        let proposal = CandidateProposal {
            principal: PrincipalId::parse("00000000000000000000000003").unwrap(),
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
        if observed {
            store
                .observe_candidate(&serde_json::to_vec(&proof).unwrap())
                .unwrap();
        }
        Self {
            store,
            root,
            trust,
            authority_key,
            authority_request,
            verifier,
            binding,
            key,
            operation,
            proof,
        }
    }

    fn begin(&mut self) -> (AssertionRequest, CoordinatedPending) {
        self.store
            .begin_member_assertion(&self.verifier, &self.binding, &self.operation)
            .unwrap()
    }

    fn assert_binding_unchanged(&self) {
        assert_eq!(fixture_state(&self.binding), (0, 0, false));
    }
}

fn all_capabilities() -> std::collections::BTreeSet<OrganizationCapability> {
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

fn expired_time() -> StoreResult<u64> {
    crate::enrollment_proof::current_time()
        .map(|now| now + 301)
        .map_err(|_| StoreError::Unavailable)
}

fn before_time() -> StoreResult<u64> {
    crate::enrollment_proof::current_time()
        .map(|now| now - 301)
        .map_err(|_| StoreError::Unavailable)
}

fn unavailable_time() -> StoreResult<u64> {
    Err(StoreError::Unavailable)
}

#[test]
fn stored_candidate_and_member_assertion_share_a_current_authority_view() {
    let mut fixture = Fixture::new(true);
    let checkpoint = fixture.store.checkpoint().unwrap();
    let (request, pending) = fixture.begin();
    fixture.assert_binding_unchanged();
    let assertion = response(&request, &fixture.binding, &fixture.key);
    let checked = fixture
        .store
        .finish_member_assertion(&fixture.verifier, pending, &mut fixture.binding, &assertion)
        .unwrap();
    assert_eq!(checked.checkpoint(), checkpoint);
    assert_eq!(fixture_state(&fixture.binding), (1, 1, false));
    assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
    assert!(fixture
        .store
        .candidate(&fixture.operation)
        .unwrap()
        .proof
        .is_some());
}

#[test]
fn unobserved_unknown_or_foreign_candidates_do_not_start_a_ceremony() {
    let mut fixture = Fixture::new(false);
    assert!(matches!(
        fixture.store.begin_member_assertion(
            &fixture.verifier,
            &fixture.binding,
            &fixture.operation
        ),
        Err(StoreError::Refused)
    ));
    let unknown = OperationId::parse("00000000000000000000000099").unwrap();
    assert!(matches!(
        fixture
            .store
            .begin_member_assertion(&fixture.verifier, &fixture.binding, &unknown),
        Err(StoreError::Refused)
    ));
    fixture
        .store
        .observe_candidate(&serde_json::to_vec(&fixture.proof).unwrap())
        .unwrap();
    let foreign = fixture_binding_with_identity(
        OrganizationId::parse("00000000000000000000000009").unwrap(),
        fixture.trust.initial_principal.clone(),
    );
    assert!(matches!(
        fixture
            .store
            .begin_member_assertion(&fixture.verifier, &foreign, &fixture.operation),
        Err(StoreError::Refused)
    ));
    let absent = fixture_binding_with_identity(
        fixture.trust.organization.clone(),
        PrincipalId::parse("00000000000000000000000030").unwrap(),
    );
    assert!(matches!(
        fixture
            .store
            .begin_member_assertion(&fixture.verifier, &absent, &fixture.operation),
        Err(StoreError::Refused)
    ));
    fixture.assert_binding_unchanged();
    fixture.store.checkpoint().unwrap();
}

#[test]
fn a_member_without_member_administration_or_a_revoked_principal_cannot_begin() {
    for revoked in [false, true] {
        let mut fixture = Fixture::new(true);
        let action = if revoked {
            Action::RevokePrincipal {
                principal: fixture.trust.initial_principal.clone(),
            }
        } else {
            Action::SetOrganizationCapabilities {
                principal: fixture.trust.initial_principal.clone(),
                capabilities: Default::default(),
            }
        };
        apply_change(
            &mut fixture.store,
            &fixture.authority_request,
            &fixture.authority_key,
            action,
        );
        assert!(matches!(
            fixture.store.begin_member_assertion(
                &fixture.verifier,
                &fixture.binding,
                &fixture.operation
            ),
            Err(StoreError::Refused)
        ));
        fixture.assert_binding_unchanged();
    }
}

#[test]
fn permission_loss_or_revocation_after_start_refuses_without_mutating_the_credential() {
    for revoked in [false, true] {
        let mut fixture = Fixture::new(true);
        let (request, pending) = fixture.begin();
        let assertion = response(&request, &fixture.binding, &fixture.key);
        let action = if revoked {
            Action::RevokePrincipal {
                principal: fixture.trust.initial_principal.clone(),
            }
        } else {
            Action::SetOrganizationCapabilities {
                principal: fixture.trust.initial_principal.clone(),
                capabilities: Default::default(),
            }
        };
        apply_change(
            &mut fixture.store,
            &fixture.authority_request,
            &fixture.authority_key,
            action,
        );
        assert!(matches!(
            fixture.store.finish_member_assertion(
                &fixture.verifier,
                pending,
                &mut fixture.binding,
                &assertion
            ),
            Err(StoreError::Refused)
        ));
        fixture.assert_binding_unchanged();
    }
}

#[test]
fn another_handle_advances_the_high_water_mark_even_when_the_old_ceremony_refuses() {
    let mut fixture = Fixture::new(true);
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.key);
    let checkpoint = fixture.store.checkpoint().unwrap();
    let mut other =
        AuthorityStore::open_existing(fd(&fixture.root), fixture.trust.clone(), checkpoint)
            .unwrap();
    apply_change(
        &mut other,
        &fixture.authority_request,
        &fixture.authority_key,
        Action::CreateProject {
            project: ProjectId::parse("00000000000000000000000080").unwrap(),
        },
    );
    let current = other.checkpoint().unwrap();
    assert!(matches!(
        fixture.store.finish_member_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion
        ),
        Err(StoreError::Refused)
    ));
    assert_eq!(fixture.store.expected, current);
    fixture.assert_binding_unchanged();
}

#[test]
fn a_conflicting_digest_refuses_even_at_the_same_revision() {
    let mut fixture = Fixture::new(true);
    let (request, mut pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.key);
    pending.checkpoint.digest[0] ^= 1;
    assert!(matches!(
        fixture.store.finish_member_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion
        ),
        Err(StoreError::Refused)
    ));
    fixture.assert_binding_unchanged();
}

#[test]
fn restoring_the_same_capability_does_not_renew_an_old_ceremony() {
    let mut fixture = Fixture::build(true, true);
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.key);
    apply_change(
        &mut fixture.store,
        &fixture.authority_request,
        &fixture.authority_key,
        Action::SetOrganizationCapabilities {
            principal: fixture.trust.initial_principal.clone(),
            capabilities: [OrganizationCapability::ManageOrganizationCapabilities]
                .into_iter()
                .collect(),
        },
    );
    let mut other_admin = fixture.authority_request.clone();
    other_admin.change.actor = PrincipalId::parse("00000000000000000000000003").unwrap();
    other_admin.device = DeviceId::parse("00000000000000000000000008").unwrap();
    other_admin.credential = CredentialId::parse("00000000000000000000000009").unwrap();
    apply_change(
        &mut fixture.store,
        &other_admin,
        &fixture.authority_key,
        Action::SetOrganizationCapabilities {
            principal: fixture.trust.initial_principal.clone(),
            capabilities: all_capabilities(),
        },
    );
    assert!(fixture.store.replay().unwrap().0.can_manage_members(
        &fixture.trust.organization,
        &fixture.trust.initial_principal
    ));
    assert!(matches!(
        fixture.store.finish_member_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion
        ),
        Err(StoreError::Refused)
    ));
    fixture.assert_binding_unchanged();
}

#[test]
fn a_changed_stored_proof_refuses_even_when_the_checkpoint_is_unchanged() {
    let mut fixture = Fixture::new(true);
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.key);
    let checkpoint = fixture.store.checkpoint().unwrap();
    let mut challenge = fixture.proof.challenge.clone();
    challenge.expires_at -= 1;
    let changed = Proof::signed(
        challenge,
        &fixture.key,
        crate::enrollment_proof::current_time().unwrap(),
    )
    .unwrap();
    let mut statement = fixture
        .store
        .database
        .prepare("UPDATE pending SET challenge=?1,proof=?2 WHERE operation=?3")
        .unwrap();
    statement
        .bind_blob(
            1,
            &super::super::canonical_json(&changed.challenge).unwrap(),
        )
        .unwrap();
    statement
        .bind_blob(2, &super::super::canonical_json(&changed).unwrap())
        .unwrap();
    statement
        .bind_blob(3, fixture.operation.as_str().as_bytes())
        .unwrap();
    statement.done().unwrap();
    drop(statement);
    assert_eq!(fixture.store.checkpoint().unwrap(), checkpoint);
    assert!(matches!(
        fixture.store.finish_member_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion
        ),
        Err(StoreError::Refused)
    ));
    fixture.assert_binding_unchanged();
}

#[test]
fn a_copied_database_cannot_receive_a_token_issued_by_another_physical_store() {
    let mut fixture = Fixture::new(true);
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.key);
    let checkpoint = fixture.store.checkpoint().unwrap();
    let copied_root = root();
    std::fs::copy(
        fixture.root.path().join("authority.sqlite3"),
        copied_root.path().join("authority.sqlite3"),
    )
    .unwrap();
    let mut copied =
        AuthorityStore::open_existing(fd(&copied_root), fixture.trust.clone(), checkpoint).unwrap();
    assert!(matches!(
        copied.finish_member_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &assertion
        ),
        Err(StoreError::Refused)
    ));
    fixture.assert_binding_unchanged();
}

#[test]
fn another_handle_to_the_same_physical_store_can_finish_the_checked_view() {
    let mut fixture = Fixture::new(true);
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.key);
    let checkpoint = fixture.store.checkpoint().unwrap();
    let mut other =
        AuthorityStore::open_existing(fd(&fixture.root), fixture.trust.clone(), checkpoint)
            .unwrap();
    assert_eq!(
        other
            .finish_member_assertion(&fixture.verifier, pending, &mut fixture.binding, &assertion)
            .unwrap()
            .checkpoint(),
        checkpoint
    );
    assert_eq!(fixture_state(&fixture.binding), (1, 1, false));
}

#[test]
fn invalid_assertions_leave_the_binding_unchanged_and_allow_a_new_ceremony() {
    let mut fixture = Fixture::new(true);
    let (request, pending) = fixture.begin();
    let mut assertion: serde_json::Value =
        serde_json::from_slice(&response(&request, &fixture.binding, &fixture.key)).unwrap();
    assertion["response"]["signature"] =
        serde_json::Value::String(passki::Passki::base64_encode(&[0; 64]));
    assert!(matches!(
        fixture.store.finish_member_assertion(
            &fixture.verifier,
            pending,
            &mut fixture.binding,
            &serde_json::to_vec(&assertion).unwrap()
        ),
        Err(StoreError::Refused)
    ));
    fixture.assert_binding_unchanged();
    let (request, pending) = fixture.begin();
    let assertion = response(&request, &fixture.binding, &fixture.key);
    fixture
        .store
        .finish_member_assertion(&fixture.verifier, pending, &mut fixture.binding, &assertion)
        .unwrap();
    assert_eq!(fixture_state(&fixture.binding), (1, 1, false));
}

#[test]
fn clock_expiry_backwards_and_unavailability_refuse_without_publication() {
    for clock in [
        expired_time as fn() -> StoreResult<u64>,
        before_time,
        unavailable_time,
    ] {
        let mut fixture = Fixture::new(true);
        assert!(fixture
            .store
            .begin_member_assertion_using(
                &fixture.verifier,
                &fixture.binding,
                &fixture.operation,
                clock
            )
            .is_err());
        let (request, pending) = fixture.begin();
        let assertion = response(&request, &fixture.binding, &fixture.key);
        assert!(fixture
            .store
            .finish_member_assertion_using(
                &fixture.verifier,
                pending,
                &mut fixture.binding,
                &assertion,
                clock
            )
            .is_err());
        fixture.assert_binding_unchanged();
        fixture.store.checkpoint().unwrap();
    }
}

#[test]
fn commit_boundary_failures_do_not_publish_or_mutate_a_prepared_credential() {
    for begin in [false, true] {
        for point in [FaultPoint::BeforeCommit, FaultPoint::AfterCommit] {
            let mut fixture = Fixture::new(true);
            let checkpoint = fixture.store.checkpoint().unwrap();
            let setup = if begin { None } else { Some(fixture.begin()) };
            fixture.store.set_fault_hook(Some(match point {
                FaultPoint::BeforeCommit => |seen| seen == FaultPoint::BeforeCommit,
                FaultPoint::AfterCommit => |seen| seen == FaultPoint::AfterCommit,
                _ => unreachable!(),
            }));
            let error = if let Some((request, pending)) = setup {
                let assertion = response(&request, &fixture.binding, &fixture.key);
                fixture
                    .store
                    .finish_member_assertion(
                        &fixture.verifier,
                        pending,
                        &mut fixture.binding,
                        &assertion,
                    )
                    .err()
                    .unwrap()
            } else {
                fixture
                    .store
                    .begin_member_assertion(&fixture.verifier, &fixture.binding, &fixture.operation)
                    .err()
                    .unwrap()
            };
            assert_eq!(
                error,
                if point == FaultPoint::BeforeCommit {
                    StoreError::Unpublished
                } else {
                    StoreError::CommitUnknown
                }
            );
            fixture.assert_binding_unchanged();
            if point == FaultPoint::AfterCommit {
                assert_eq!(fixture.store.checkpoint(), Err(StoreError::CommitUnknown));
            }
            let mut reopened =
                AuthorityStore::open_existing(fd(&fixture.root), fixture.trust.clone(), checkpoint)
                    .unwrap();
            assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
            assert!(reopened
                .candidate(&fixture.operation)
                .unwrap()
                .proof
                .is_some());
        }
    }
}

#[test]
fn an_existing_writer_refuses_both_steps_without_partial_credential_state() {
    for begin in [false, true] {
        let mut fixture = Fixture::new(true);
        let checkpoint = fixture.store.checkpoint().unwrap();
        let setup = if begin { None } else { Some(fixture.begin()) };
        let other =
            AuthorityStore::open_existing(fd(&fixture.root), fixture.trust.clone(), checkpoint)
                .unwrap();
        other.database.execute("BEGIN IMMEDIATE").unwrap();
        let error = if let Some((request, pending)) = setup {
            let assertion = response(&request, &fixture.binding, &fixture.key);
            fixture
                .store
                .finish_member_assertion(
                    &fixture.verifier,
                    pending,
                    &mut fixture.binding,
                    &assertion,
                )
                .err()
                .unwrap()
        } else {
            fixture
                .store
                .begin_member_assertion(&fixture.verifier, &fixture.binding, &fixture.operation)
                .err()
                .unwrap()
        };
        assert_eq!(error, StoreError::Unpublished);
        fixture.assert_binding_unchanged();
        other.database.execute("ROLLBACK").unwrap();
        fixture.store.checkpoint().unwrap();
    }
}
