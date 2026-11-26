use super::*;
use crate::approval::durable::PublicPasskeyState;
use crate::approval::tests::{fixture as assertion_fixture, response};
use crate::approval::TrustedPasskeyBinding;
use crate::durable::pending::coordinator::additional::existing_client_message;
use crate::durable::registered_assertion::registered_policy_commitment;
use crate::durable::tests::{fd, fixture as authority_fixture, root, TestRoot};
use crate::enrollment_proof::decode_hex;
use crate::team_authority::signed::v2::registry::admission::FixturePasskeyAssociation;
use crate::team_authority::signed::v2::V2Action;
use ed25519_dalek::{Signer, SigningKey};
use passki::StoredPasskey;
use std::fs;

fn id(value: u64) -> String {
    format!("{value:026}")
}
fn device(value: u64) -> DeviceId {
    DeviceId::parse(&id(value)).unwrap()
}
fn credential(value: u64) -> CredentialId {
    CredentialId::parse(&id(value)).unwrap()
}

fn public_passkey(key: &SigningKey) -> StoredPasskey {
    let mut public = Vec::new();
    ciborium::into_writer(
        &ciborium::Value::Map(vec![
            (
                ciborium::Value::Integer(1.into()),
                ciborium::Value::Integer(1.into()),
            ),
            (
                ciborium::Value::Integer(3.into()),
                ciborium::Value::Integer((-8).into()),
            ),
            (
                ciborium::Value::Integer((-1).into()),
                ciborium::Value::Integer(6.into()),
            ),
            (
                ciborium::Value::Integer((-2).into()),
                ciborium::Value::Bytes(key.verifying_key().to_bytes().to_vec()),
            ),
        ]),
        &mut public,
    )
    .unwrap();
    let mut passkey = StoredPasskey::default();
    passkey.credential_id = vec![0x21; 32];
    passkey.public_key = public;
    passkey.algorithm = -8;
    passkey
}

struct Fixture {
    store: AuthorityStoreEnrollment,
    trust: FixtureEnrollmentTrust,
    old_key: SigningKey,
    candidate_key: SigningKey,
    passkey_binding: TrustedPasskeyBinding,
    root: TestRoot,
}

impl Fixture {
    fn new() -> Self {
        let root = root();
        let (base, old_key, _) = authority_fixture();
        let (_, passkey_binding, candidate_key, _) = assertion_fixture();
        let state = PublicPasskeyState::new_fixture(
            base.organization.clone(),
            base.initial_principal.clone(),
            public_passkey(&candidate_key),
        )
        .unwrap();
        let trust = FixtureEnrollmentTrust {
            registered: FixtureRegisteredTrust {
                policy_admin: base.initial_principal.clone(),
                base,
            },
            rp_id: "vivac.test".into(),
            origin: "https://vivac.test".into(),
            associations: vec![FixturePasskeyAssociation {
                device: device(3),
                credential: credential(4),
                state,
            }],
        };
        let store = AuthorityStoreEnrollment::create_new(fd(&root), trust.clone()).unwrap();
        let mut fixture = Self {
            store,
            trust,
            old_key,
            candidate_key,
            passkey_binding,
            root,
        };
        fixture.policy(V2Action::SetOwnAdditionalDevice {
            principal: fixture.trust.registered.base.initial_principal.clone(),
            enabled: true,
        });
        fixture
    }
    fn policy(&mut self, action: V2Action) {
        let revision = self.store.checkpoint().unwrap().revision;
        let request = DeviceRequestV2 {
            organization: self.trust.registered.base.organization.clone(),
            actor: self.trust.registered.base.initial_principal.clone(),
            device: device(3),
            credential: credential(4),
            operation: OperationId::parse(&id(10 + revision)).unwrap(),
            base_revision: revision,
            action,
        };
        self.store
            .admit(&EnrollmentRequest::policy(SignedRequestV2 {
                signature: self.old_key.sign(&request.canonical_bytes()).to_bytes(),
                request,
            }))
            .unwrap();
    }
    fn begin(&mut self) -> (AssertionRequest, PendingEnrollment, Proof) {
        let (request, pending, proof, _) = self.begin_with_endorsement();
        (request, pending, proof)
    }
    fn begin_with_endorsement(
        &mut self,
    ) -> (
        AssertionRequest,
        PendingEnrollment,
        Proof,
        ExistingClientEndorsement,
    ) {
        let proposal = CandidateProposal {
            principal: self.trust.registered.base.initial_principal.clone(),
            device: device(20),
            credential: credential(21),
            public_key: self.candidate_key.verifying_key().to_bytes(),
            lifetime_secs: 300,
        };
        let (challenge, issued) = self.store.issue_additional_enrollment(&proposal).unwrap();
        let proof = Proof::signed(challenge, &self.candidate_key, system_time().unwrap()).unwrap();
        let policy = registered_policy_commitment(
            &self.trust.registered.base.organization,
            &self.trust.registered.base.initial_principal,
            issued.checkpoint,
        );
        let message = existing_client_message(
            &self.trust.registered.base.organization,
            &self.trust.registered.base.initial_principal,
            &device(3),
            &credential(4),
            issued.checkpoint,
            policy,
            &proof.challenge,
        )
        .unwrap();
        let endorsement = ExistingClientEndorsement {
            device: device(3),
            credential: credential(4),
            signature: self.old_key.sign(&message).to_bytes(),
        };
        let (request, pending) = self
            .store
            .begin_additional_enrollment(issued, &proof, &endorsement)
            .unwrap();
        (request, pending, proof, endorsement)
    }
    fn state(&mut self) -> PublicPasskeyState {
        self.store
            .passkey_state(
                &self.trust.registered.base.organization,
                &self.trust.registered.base.initial_principal,
                &device(3),
                &credential(4),
            )
            .unwrap()
    }
}

#[test]
fn atomic_enrollment_survives_restart_with_counter_and_consumed_nonce() {
    let mut fixture = Fixture::new();
    let (request, pending, proof) = fixture.begin();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    assert_eq!(
        fixture
            .store
            .finish_additional_enrollment(pending, &assertion)
            .unwrap(),
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 2
        }
    );
    let retained = fixture.store.checkpoint().unwrap();
    let nonce = decode_hex::<32>(&proof.challenge.nonce).unwrap();
    let mut reopened =
        AuthorityStoreEnrollment::open_existing(fd(&fixture.root), fixture.trust.clone(), retained)
            .unwrap();
    assert!(reopened
        .binding_is_active(
            &fixture.trust.registered.base.organization,
            &fixture.trust.registered.base.initial_principal,
            &device(20),
            &credential(21)
        )
        .unwrap());
    let state = reopened
        .passkey_state(
            &fixture.trust.registered.base.organization,
            &fixture.trust.registered.base.initial_principal,
            &device(3),
            &credential(4),
        )
        .unwrap();
    assert_eq!((state.counter(), state.generation()), (1, 1));
    assert!(reopened.enrollment_nonce_consumed(nonce).unwrap());
    let request = DeviceRequestV2 {
        organization: fixture.trust.registered.base.organization.clone(),
        actor: fixture.trust.registered.base.initial_principal.clone(),
        device: device(20),
        credential: credential(21),
        operation: OperationId::parse(&id(50)).unwrap(),
        base_revision: 2,
        action: V2Action::SetOwnAdditionalDevice {
            principal: fixture.trust.registered.base.initial_principal.clone(),
            enabled: false,
        },
    };
    assert_eq!(
        reopened
            .admit(&EnrollmentRequest::policy(SignedRequestV2 {
                signature: fixture
                    .candidate_key
                    .sign(&request.canonical_bytes())
                    .to_bytes(),
                request
            }))
            .unwrap()
            .revision,
        3
    );
}

fn archive(store: &AuthorityStoreEnrollment) -> EnrollmentRequest {
    let mut row = store
        .database
        .prepare("SELECT request,signature FROM entries ORDER BY seq DESC LIMIT 1")
        .unwrap();
    assert!(row.row().unwrap());
    let bytes = row.blob(0, MAX_ADMISSION_BYTES).unwrap();
    let signature = row.blob(1, 64).unwrap().try_into().unwrap();
    EnrollmentProfile::parse_signed(&bytes, signature).unwrap()
}

fn finish(fixture: &mut Fixture) -> (EnrollmentRequest, Proof) {
    let (request, pending, proof) = fixture.begin();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    fixture
        .store
        .finish_additional_enrollment(pending, &assertion)
        .unwrap();
    (archive(&fixture.store), proof)
}

fn fault_insert(point: FaultPoint) -> bool {
    point == FaultPoint::AfterInsert
}
fn fault_before(point: FaultPoint) -> bool {
    point == FaultPoint::BeforeCommit
}
fn fault_after(point: FaultPoint) -> bool {
    point == FaultPoint::AfterCommit
}

#[test]
fn lost_commit_acknowledgement_reconciles_without_reapproval_or_counter_renewal() {
    let mut fixture = Fixture::new();
    let before = fixture.store.checkpoint().unwrap();
    let (request, pending, proof, endorsement) = fixture.begin_with_endorsement();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    fixture.store.set_fault_hook(Some(fault_after));
    assert_eq!(
        fixture
            .store
            .finish_additional_enrollment(pending, &assertion),
        Err(StoreError::CommitUnknown)
    );
    assert_eq!(fixture.store.checkpoint(), Err(StoreError::CommitUnknown));
    let mut reopened =
        AuthorityStoreEnrollment::open_existing(fd(&fixture.root), fixture.trust.clone(), before)
            .unwrap();
    assert_eq!(
        reopened
            .reconcile_additional_enrollment(&proof, &endorsement)
            .unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 2
        }
    );
    let state = reopened
        .passkey_state(
            &fixture.trust.registered.base.organization,
            &fixture.trust.registered.base.initial_principal,
            &device(3),
            &credential(4),
        )
        .unwrap();
    assert_eq!((state.counter(), state.generation()), (1, 1));
    assert!(reopened
        .enrollment_nonce_consumed(decode_hex::<32>(&proof.challenge.nonce).unwrap())
        .unwrap());
    assert_eq!(reopened.checkpoint().unwrap().revision, 2);
}

#[test]
fn reconciliation_refuses_unknown_and_rolled_back_enrollments() {
    let mut fixture = Fixture::new();
    let (request, pending, proof, endorsement) = fixture.begin_with_endorsement();
    let before = fixture.store.checkpoint().unwrap();
    assert_eq!(
        fixture
            .store
            .reconcile_additional_enrollment(&proof, &endorsement),
        Err(StoreError::Refused)
    );
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    fixture.store.set_fault_hook(Some(fault_before));
    assert_eq!(
        fixture
            .store
            .finish_additional_enrollment(pending, &assertion),
        Err(StoreError::Unpublished)
    );
    fixture.store.set_fault_hook(None);
    assert_eq!(
        fixture
            .store
            .reconcile_additional_enrollment(&proof, &endorsement),
        Err(StoreError::Refused)
    );
    assert_eq!(fixture.store.checkpoint().unwrap(), before);
    assert_eq!(
        (fixture.state().counter(), fixture.state().generation()),
        (0, 0)
    );
    assert!(!fixture
        .store
        .enrollment_nonce_consumed(decode_hex::<32>(&proof.challenge.nonce).unwrap())
        .unwrap());
}

#[test]
fn reconciliation_requires_exact_original_proof_and_endorsement_without_writes() {
    let mut fixture = Fixture::new();
    let (request, pending, proof, endorsement) = fixture.begin_with_endorsement();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    fixture
        .store
        .finish_additional_enrollment(pending, &assertion)
        .unwrap();
    let before = fixture.store.checkpoint().unwrap();
    let state = fixture.state().commitment();
    let database = fixture.root.path().join("authority.sqlite3");
    let bytes = fs::read(&database).unwrap();
    for change in [
        "candidate",
        "proof_signature",
        "proof_status",
        "endorsement_signature",
        "other_current_binding",
    ] {
        let mut proof = proof.clone();
        let mut endorsement = endorsement.clone();
        match change {
            "candidate" => proof.challenge.device_id = id(22),
            "proof_signature" => {
                let mut signature = decode_hex::<64>(&proof.signature).unwrap();
                signature[0] ^= 1;
                proof.signature = hex(&signature);
            }
            "proof_status" => proof.status = "approved".into(),
            "endorsement_signature" => endorsement.signature[0] ^= 1,
            "other_current_binding" => {
                endorsement.device = device(20);
                endorsement.credential = credential(21);
            }
            _ => unreachable!(),
        }
        assert_eq!(
            fixture
                .store
                .reconcile_additional_enrollment(&proof, &endorsement),
            Err(StoreError::Refused),
            "{change}"
        );
        assert_eq!(fixture.store.checkpoint().unwrap(), before);
        assert_eq!(fixture.state().commitment(), state);
    }
    assert_eq!(
        fixture
            .store
            .reconcile_additional_enrollment(&proof, &endorsement)
            .unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 2
        }
    );
    assert_eq!(fixture.state().commitment(), state);
    assert_eq!(fs::read(database).unwrap(), bytes);
}

#[test]
fn reconciliation_refuses_a_revoked_original_credential() {
    let mut fixture = Fixture::new();
    let (request, pending, proof, endorsement) = fixture.begin_with_endorsement();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    fixture
        .store
        .finish_additional_enrollment(pending, &assertion)
        .unwrap();
    let revoke = crate::team_authority::signed::v2::registry::RevocationRequest {
        organization: fixture.trust.registered.base.organization.clone(),
        actor: fixture.trust.registered.base.initial_principal.clone(),
        device: device(20),
        credential: credential(21),
        operation: OperationId::parse(&id(90)).unwrap(),
        base_revision: 2,
        target_device: device(3),
        target_credential: credential(4),
    };
    fixture
        .store
        .admit(&EnrollmentRequest::revoke(SignedRevocation {
            signature: fixture
                .candidate_key
                .sign(&revoke.canonical_bytes())
                .to_bytes(),
            request: revoke,
        }))
        .unwrap();
    let before = fixture.store.checkpoint().unwrap();
    let database = fixture.root.path().join("authority.sqlite3");
    let bytes = fs::read(&database).unwrap();
    assert_eq!(
        fixture
            .store
            .reconcile_additional_enrollment(&proof, &endorsement),
        Err(StoreError::Refused)
    );
    assert_eq!(fixture.store.checkpoint().unwrap(), before);
    assert_eq!(fs::read(database).unwrap(), bytes);
}

#[test]
fn transaction_failures_leave_binding_counter_and_consumption_unchanged() {
    for fault in [fault_insert as fn(FaultPoint) -> bool, fault_before] {
        let mut fixture = Fixture::new();
        let before = fixture.store.checkpoint().unwrap();
        let state = fixture.state().commitment();
        let (request, pending, proof) = fixture.begin();
        let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
        fixture.store.set_fault_hook(Some(fault));
        assert_eq!(
            fixture
                .store
                .finish_additional_enrollment(pending, &assertion),
            Err(StoreError::Unpublished)
        );
        fixture.store.set_fault_hook(None);
        assert_eq!(fixture.store.checkpoint().unwrap(), before);
        assert_eq!(fixture.state().commitment(), state);
        assert!(!fixture
            .store
            .binding_is_active(
                &fixture.trust.registered.base.organization,
                &fixture.trust.registered.base.initial_principal,
                &device(20),
                &credential(21)
            )
            .unwrap());
        assert!(!fixture
            .store
            .enrollment_nonce_consumed(decode_hex::<32>(&proof.challenge.nonce).unwrap())
            .unwrap());
    }
}

#[test]
fn decoded_witness_cannot_create_a_live_enrollment_in_another_store() {
    let mut fixture = Fixture::new();
    let (request, _) = finish(&mut fixture);
    let mut other = Fixture::new();
    let before = other.store.checkpoint().unwrap();
    assert_eq!(other.store.admit(&request), Err(StoreError::Refused));
    assert_eq!(other.store.checkpoint().unwrap(), before);
    assert_eq!(other.state().counter(), 0);
    assert_eq!(
        fixture.store.admit(&request).unwrap().admission,
        Admission::Historical
    );
    assert_eq!(fixture.state().counter(), 1);
}

#[test]
fn a_changed_policy_invalidates_pending_enrollment_even_after_regrant() {
    let mut fixture = Fixture::new();
    let (request, pending, _) = fixture.begin();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    fixture.policy(V2Action::SetOwnAdditionalDevice {
        principal: fixture.trust.registered.base.initial_principal.clone(),
        enabled: false,
    });
    fixture.policy(V2Action::SetOwnAdditionalDevice {
        principal: fixture.trust.registered.base.initial_principal.clone(),
        enabled: true,
    });
    let before = fixture.store.checkpoint().unwrap();
    assert_eq!(
        fixture
            .store
            .finish_additional_enrollment(pending, &assertion),
        Err(StoreError::Refused)
    );
    assert_eq!(fixture.store.checkpoint().unwrap(), before);
    assert_eq!(fixture.state().generation(), 0);
}

#[test]
fn another_handle_rechecks_the_registry_before_committing_an_enrollment() {
    let mut fixture = Fixture::new();
    let before = fixture.store.checkpoint().unwrap();
    let mut other =
        AuthorityStoreEnrollment::open_existing(fd(&fixture.root), fixture.trust.clone(), before)
            .unwrap();
    let (request, pending, _) = fixture.begin();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    fixture.policy(V2Action::SetOwnAdditionalDevice {
        principal: fixture.trust.registered.base.initial_principal.clone(),
        enabled: false,
    });
    assert_eq!(
        other.finish_additional_enrollment(pending, &assertion),
        Err(StoreError::Refused)
    );
    assert_eq!(
        other.checkpoint().unwrap(),
        fixture.store.checkpoint().unwrap()
    );
    assert_eq!(fixture.state().counter(), 0);
}

#[test]
fn revoking_the_prior_credential_blocks_finish_and_historical_reconciliation() {
    let mut fixture = Fixture::new();
    let (archived, _) = finish(&mut fixture);
    let request = crate::team_authority::signed::v2::registry::RevocationRequest {
        organization: fixture.trust.registered.base.organization.clone(),
        actor: fixture.trust.registered.base.initial_principal.clone(),
        device: device(20),
        credential: credential(21),
        operation: OperationId::parse(&id(90)).unwrap(),
        base_revision: 2,
        target_device: device(3),
        target_credential: credential(4),
    };
    fixture
        .store
        .admit(&EnrollmentRequest::revoke(SignedRevocation {
            signature: fixture
                .candidate_key
                .sign(&request.canonical_bytes())
                .to_bytes(),
            request,
        }))
        .unwrap();
    assert_eq!(fixture.store.admit(&archived), Err(StoreError::Refused));
    assert_eq!(fixture.store.checkpoint().unwrap().revision, 3);
}

#[test]
fn a_parallel_ceremony_cannot_commit_against_an_obsolete_counter_and_checkpoint() {
    let mut fixture = Fixture::new();
    let (first, first_pending, _) = fixture.begin();
    let (second, second_pending, _) = fixture.begin();
    let first_assertion = response(&first, &fixture.passkey_binding, &fixture.candidate_key);
    let second_assertion = response(&second, &fixture.passkey_binding, &fixture.candidate_key);
    fixture
        .store
        .finish_additional_enrollment(first_pending, &first_assertion)
        .unwrap();
    assert_eq!(
        fixture
            .store
            .finish_additional_enrollment(second_pending, &second_assertion),
        Err(StoreError::Refused)
    );
    assert_eq!(
        (fixture.state().counter(), fixture.state().generation()),
        (1, 1)
    );
}

#[test]
fn monotonic_expiration_refuses_issued_and_pending_tokens_without_sleeping() {
    let mut fixture = Fixture::new();
    let proposal = CandidateProposal {
        principal: fixture.trust.registered.base.initial_principal.clone(),
        device: device(20),
        credential: credential(21),
        public_key: fixture.candidate_key.verifying_key().to_bytes(),
        lifetime_secs: 300,
    };
    let (challenge, mut issued) = fixture
        .store
        .issue_additional_enrollment(&proposal)
        .unwrap();
    let proof = Proof::signed(challenge, &fixture.candidate_key, system_time().unwrap()).unwrap();
    issued.deadline = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .unwrap();
    let endorsement = ExistingClientEndorsement {
        device: device(3),
        credential: credential(4),
        signature: [0; 64],
    };
    assert!(matches!(
        fixture
            .store
            .begin_additional_enrollment(issued, &proof, &endorsement),
        Err(StoreError::Refused)
    ));
    let (request, mut pending, _) = fixture.begin();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    pending.issued.deadline = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        fixture
            .store
            .finish_additional_enrollment(pending, &assertion),
        Err(StoreError::Refused)
    );
    assert_eq!(fixture.store.checkpoint().unwrap().revision, 1);
    assert_eq!(fixture.state().counter(), 0);
}

#[test]
fn a_token_cannot_be_transferred_to_a_different_physical_store() {
    let mut fixture = Fixture::new();
    let mut other = Fixture::new();
    let (request, pending, _) = fixture.begin();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    assert_eq!(
        other
            .store
            .finish_additional_enrollment(pending, &assertion),
        Err(StoreError::Refused)
    );
    assert_eq!(other.state().counter(), 0);
    assert_eq!(other.store.checkpoint().unwrap().revision, 1);
}

#[test]
fn malformed_assertions_and_mismatched_candidates_do_not_change_durable_state() {
    for change in ["signature", "proof", "endorsement"] {
        let mut fixture = Fixture::new();
        let (request, mut pending, _) = fixture.begin();
        let mut assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
        match change {
            "signature" => assertion = vec![0; 16385],
            "proof" => pending.proof.challenge.device_id = id(22),
            "endorsement" => pending.endorsement.signature[0] ^= 1,
            _ => unreachable!(),
        }
        assert_eq!(
            fixture
                .store
                .finish_additional_enrollment(pending, &assertion),
            Err(StoreError::Refused)
        );
        assert_eq!(fixture.store.checkpoint().unwrap().revision, 1);
        assert_eq!(fixture.state().counter(), 0);
    }
}

#[test]
fn schema_and_row_tampering_are_refused_after_an_admitted_enrollment() {
    for tamper in [
        "UPDATE entries SET signature=zeroblob(64) WHERE seq=2",
        "UPDATE entries SET operation=CAST('00000000000000000000000099' AS BLOB) WHERE seq=2",
        "UPDATE meta SET head=zeroblob(32)",
        "CREATE TABLE unexpected (value BLOB) STRICT",
    ] {
        let mut fixture = Fixture::new();
        finish(&mut fixture);
        fixture.store.database.execute(tamper).unwrap();
        assert_eq!(fixture.store.checkpoint(), Err(StoreError::Refused));
    }
}

#[test]
fn invalid_bootstrap_associations_and_origins_never_create_the_database() {
    for change in ["empty", "duplicate", "owner", "origin"] {
        let fixture = Fixture::new();
        let root = root();
        let mut trust = fixture.trust.clone();
        match change {
            "empty" => trust.associations.clear(),
            "duplicate" => trust.associations.push(trust.associations[0].clone()),
            "owner" => trust.associations[0].device = device(99),
            "origin" => trust.origin = "http://vivac.test".into(),
            _ => unreachable!(),
        }
        assert!(matches!(
            AuthorityStoreEnrollment::create_new(fd(&root), trust),
            Err(StoreError::Refused)
        ));
        assert!(!root.path().join("authority.sqlite3").exists());
    }
}

#[test]
fn retained_checkpoint_detects_restore_before_counter_and_nonce_consumption() {
    let mut fixture = Fixture::new();
    let database = fixture.root.path().join("authority.sqlite3");
    let old = fs::read(&database).unwrap();
    finish(&mut fixture);
    let retained = fixture.store.checkpoint().unwrap();
    drop(fixture.store);
    fs::write(&database, &old).unwrap();
    assert!(matches!(
        AuthorityStoreEnrollment::open_existing(fd(&fixture.root), fixture.trust, retained),
        Err(StoreError::Refused)
    ));
}

#[test]
fn format_five_refuses_legacy_histories_and_legacy_profiles_refuse_format_five() {
    let fixture = Fixture::new();
    let checkpoint = fixture.trust.bootstrap_checkpoint().unwrap();
    let database = fixture.root.path().join("authority.sqlite3");
    let bytes = fs::read(&database).unwrap();
    assert!(matches!(
        AuthorityStore::open_existing(
            fd(&fixture.root),
            fixture.trust.registered.base.clone(),
            checkpoint
        ),
        Err(StoreError::Refused)
    ));
    assert!(matches!(
        AuthorityStoreV2::open_existing(
            fd(&fixture.root),
            FixtureTrustV2 {
                base: fixture.trust.registered.base.clone(),
                policy_admin: fixture.trust.registered.policy_admin.clone()
            },
            checkpoint
        ),
        Err(StoreError::Refused)
    ));
    assert!(matches!(
        AuthorityStoreRegistered::open_existing(
            fd(&fixture.root),
            fixture.trust.registered.clone(),
            checkpoint
        ),
        Err(StoreError::Refused)
    ));
    assert_eq!(fs::read(&database).unwrap(), bytes);
    for version in [2, 3, 4] {
        let root = root();
        let initial = match version {
            2 => {
                let store =
                    AuthorityStore::create_new(fd(&root), fixture.trust.registered.base.clone())
                        .unwrap();
                store.expected
            }
            3 => {
                let store = AuthorityStoreV2::create_new(
                    fd(&root),
                    FixtureTrustV2 {
                        base: fixture.trust.registered.base.clone(),
                        policy_admin: fixture.trust.registered.policy_admin.clone(),
                    },
                )
                .unwrap();
                store.expected
            }
            4 => {
                let store = AuthorityStoreRegistered::create_new(
                    fd(&root),
                    fixture.trust.registered.clone(),
                )
                .unwrap();
                store.expected
            }
            _ => unreachable!(),
        };
        let before = fs::read(root.path().join("authority.sqlite3")).unwrap();
        assert!(matches!(
            AuthorityStoreEnrollment::open_existing(fd(&root), fixture.trust.clone(), initial),
            Err(StoreError::Refused)
        ));
        assert_eq!(
            fs::read(root.path().join("authority.sqlite3")).unwrap(),
            before
        );
    }
}

#[test]
fn policy_and_revocation_operations_cannot_reuse_an_enrollment_operation() {
    let mut fixture = Fixture::new();
    let (_, proof) = finish(&mut fixture);
    let operation = OperationId::parse(&proof.challenge.operation_id).unwrap();
    let request = DeviceRequestV2 {
        organization: fixture.trust.registered.base.organization.clone(),
        actor: fixture.trust.registered.base.initial_principal.clone(),
        device: device(3),
        credential: credential(4),
        operation: operation.clone(),
        base_revision: 2,
        action: V2Action::SetOwnAdditionalDevice {
            principal: fixture.trust.registered.base.initial_principal.clone(),
            enabled: false,
        },
    };
    assert_eq!(
        fixture
            .store
            .admit(&EnrollmentRequest::policy(SignedRequestV2 {
                signature: fixture.old_key.sign(&request.canonical_bytes()).to_bytes(),
                request
            })),
        Err(StoreError::Refused)
    );
    let request = crate::team_authority::signed::v2::registry::RevocationRequest {
        organization: fixture.trust.registered.base.organization.clone(),
        actor: fixture.trust.registered.base.initial_principal.clone(),
        device: device(3),
        credential: credential(4),
        operation,
        base_revision: 2,
        target_device: device(20),
        target_credential: credential(21),
    };
    assert_eq!(
        fixture
            .store
            .admit(&EnrollmentRequest::revoke(SignedRevocation {
                signature: fixture.old_key.sign(&request.canonical_bytes()).to_bytes(),
                request
            })),
        Err(StoreError::Refused)
    );
    assert_eq!(fixture.store.checkpoint().unwrap().revision, 2);
}

#[test]
fn a_malformed_fixture_cose_key_never_admits_a_device() {
    let mut fixture = Fixture::new();
    let root = root();
    let mut malformed = public_passkey(&fixture.candidate_key);
    malformed.public_key = vec![0];
    fixture.trust.associations[0].state = PublicPasskeyState::new_fixture(
        fixture.trust.registered.base.organization.clone(),
        fixture.trust.registered.base.initial_principal.clone(),
        malformed,
    )
    .unwrap();
    fixture.store = AuthorityStoreEnrollment::create_new(fd(&root), fixture.trust.clone()).unwrap();
    fixture.policy(V2Action::SetOwnAdditionalDevice {
        principal: fixture.trust.registered.base.initial_principal.clone(),
        enabled: true,
    });
    let (request, pending, _) = fixture.begin();
    let assertion = response(&request, &fixture.passkey_binding, &fixture.candidate_key);
    assert_eq!(
        fixture
            .store
            .finish_additional_enrollment(pending, &assertion),
        Err(StoreError::Refused)
    );
    assert_eq!(fixture.state().counter(), 0);
    assert_eq!(fixture.store.checkpoint().unwrap().revision, 1);
}

#[test]
fn enrollment_bootstrap_matches_the_independent_public_fixture() {
    let vector: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/data/additional-admission-vector.json"
    )))
    .unwrap();
    let (base, _, _) = authority_fixture();
    let value = vector["cose"].as_str().unwrap();
    let mut passkey = StoredPasskey::default();
    passkey.credential_id = vec![0x21; 32];
    passkey.algorithm = -8;
    passkey.public_key = (0..value.len() / 2)
        .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap())
        .collect();
    let state = PublicPasskeyState::new_fixture(
        base.organization.clone(),
        base.initial_principal.clone(),
        passkey,
    )
    .unwrap();
    let trust = FixtureEnrollmentTrust {
        registered: FixtureRegisteredTrust {
            policy_admin: base.initial_principal.clone(),
            base,
        },
        rp_id: "vivac.test".into(),
        origin: "https://vivac.test".into(),
        associations: vec![FixturePasskeyAssociation {
            device: device(3),
            credential: credential(4),
            state,
        }],
    };
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    assert_eq!(
        hex(&checkpoint.digest),
        vector["checkpoint0"].as_str().unwrap()
    );
    assert_eq!(checkpoint.revision, 0);
}

#[test]
fn the_binding_limit_includes_reserved_credentials_before_requesting_approval() {
    let mut fixture = Fixture::new();
    let root = root();
    for index in 0..127u64 {
        fixture.trust.registered.base.known.push(
            KnownDevice::new(
                fixture.trust.registered.base.organization.clone(),
                fixture.trust.registered.base.initial_principal.clone(),
                device(1000 + index),
                credential(2000 + index),
                SigningKey::from_bytes(&[index as u8; 32])
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap(),
        );
    }
    fixture.store = AuthorityStoreEnrollment::create_new(fd(&root), fixture.trust.clone()).unwrap();
    fixture.policy(V2Action::SetOwnAdditionalDevice {
        principal: fixture.trust.registered.base.initial_principal.clone(),
        enabled: true,
    });
    let proposal = CandidateProposal {
        principal: fixture.trust.registered.base.initial_principal.clone(),
        device: device(20),
        credential: credential(21),
        public_key: SigningKey::from_bytes(&[0xff; 32])
            .verifying_key()
            .to_bytes(),
        lifetime_secs: 300,
    };
    assert!(matches!(
        fixture.store.issue_additional_enrollment(&proposal),
        Err(StoreError::Refused)
    ));
    assert_eq!(fixture.store.checkpoint().unwrap().revision, 1);
}

#[test]
fn a_full_mixed_sql_history_keeps_counter_nonce_and_historical_receipts() {
    let mut fixture = Fixture::new();
    let (enrollment, proof) = finish(&mut fixture);
    fixture.store.database.execute("BEGIN IMMEDIATE").unwrap();
    let (mut authority, mut checkpoint) = fixture.store.replay().unwrap();
    for index in 0..1022u64 {
        let request = DeviceRequestV2 {
            organization: fixture.trust.registered.base.organization.clone(),
            actor: fixture.trust.registered.base.initial_principal.clone(),
            device: device(3),
            credential: credential(4),
            operation: OperationId::parse(&id(5000 + index)).unwrap(),
            base_revision: checkpoint.revision,
            action: V2Action::SetOwnAdditionalDevice {
                principal: fixture.trust.registered.base.initial_principal.clone(),
                enabled: true,
            },
        };
        let signed = EnrollmentRequest::policy(SignedRequestV2 {
            signature: fixture.old_key.sign(&request.canonical_bytes()).to_bytes(),
            request,
        });
        assert_eq!(
            EnrollmentProfile::apply_at(&fixture.trust, &mut authority, &signed, checkpoint)
                .unwrap()
                .admission,
            Admission::NewlyAdmitted
        );
        let bytes = EnrollmentProfile::canonical_bytes(&signed);
        checkpoint =
            crate::durable::advance(checkpoint, &bytes, EnrollmentProfile::signature(&signed));
        let mut insert = fixture
            .store
            .database
            .prepare("INSERT INTO entries VALUES(?1,?2,?3,?4)")
            .unwrap();
        insert.bind_integer(1, checkpoint.revision).unwrap();
        insert
            .bind_blob(2, EnrollmentProfile::operation(&signed))
            .unwrap();
        insert.bind_blob(3, &bytes).unwrap();
        insert
            .bind_blob(4, EnrollmentProfile::signature(&signed))
            .unwrap();
        insert.done().unwrap();
    }
    let mut metadata = fixture
        .store
        .database
        .prepare("UPDATE meta SET revision=?1,head=?2")
        .unwrap();
    metadata.bind_integer(1, checkpoint.revision).unwrap();
    metadata.bind_blob(2, &checkpoint.digest).unwrap();
    metadata.done().unwrap();
    drop(metadata);
    fixture.store.database.execute("COMMIT").unwrap();
    let mut reopened = AuthorityStoreEnrollment::open_existing(
        fd(&fixture.root),
        fixture.trust.clone(),
        checkpoint,
    )
    .unwrap();
    assert_eq!(reopened.checkpoint().unwrap().revision, 1024);
    let state = reopened
        .passkey_state(
            &fixture.trust.registered.base.organization,
            &fixture.trust.registered.base.initial_principal,
            &device(3),
            &credential(4),
        )
        .unwrap();
    assert_eq!((state.counter(), state.generation()), (1, 1));
    assert!(reopened
        .enrollment_nonce_consumed(decode_hex::<32>(&proof.challenge.nonce).unwrap())
        .unwrap());
    assert_eq!(
        reopened.admit(&enrollment).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 2
        }
    );
    let request = DeviceRequestV2 {
        organization: fixture.trust.registered.base.organization.clone(),
        actor: fixture.trust.registered.base.initial_principal.clone(),
        device: device(3),
        credential: credential(4),
        operation: OperationId::parse(&id(9000)).unwrap(),
        base_revision: 1024,
        action: V2Action::SetOwnAdditionalDevice {
            principal: fixture.trust.registered.base.initial_principal.clone(),
            enabled: false,
        },
    };
    assert_eq!(
        reopened.admit(&EnrollmentRequest::policy(SignedRequestV2 {
            signature: fixture.old_key.sign(&request.canonical_bytes()).to_bytes(),
            request
        })),
        Err(StoreError::Refused)
    );
}

#[test]
fn physical_database_replacement_refuses_enrollment_queries() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    let database = fixture.root.path().join("authority.sqlite3");
    let bytes = fs::read(&database).unwrap();
    fs::rename(&database, fixture.root.path().join("previous")).unwrap();
    fs::write(&database, &bytes).unwrap();
    fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(fixture.store.checkpoint(), Err(StoreError::Refused));
}
