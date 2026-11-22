use super::super::tests::{fd, fixture, root};
use super::super::{FaultPoint, FixtureTrust};
use super::*;
use ed25519_dalek::{Signer, SigningKey};

fn proposal() -> (FixtureTrust, SigningKey, CandidateProposal, OperationId) {
    let (trust, key, request) = fixture();
    let proposal = CandidateProposal {
        principal: PrincipalId::parse("00000000000000000000000020").unwrap(),
        device: DeviceId::parse("00000000000000000000000021").unwrap(),
        credential: CredentialId::parse("00000000000000000000000022").unwrap(),
        public_key: key.verifying_key().to_bytes(),
        lifetime_secs: 300,
    };
    (trust, key, proposal, request.change.operation)
}

fn now() -> StoreResult<u64> {
    Ok(1000)
}
fn later() -> StoreResult<u64> {
    Ok(1299)
}
fn expired() -> StoreResult<u64> {
    Ok(1300)
}
fn before() -> StoreResult<u64> {
    Ok(999)
}
fn unavailable_time() -> StoreResult<u64> {
    Err(StoreError::Unavailable)
}
fn unavailable_entropy(_: &mut [u8]) -> StoreResult<()> {
    Err(StoreError::Unavailable)
}
fn fixed_entropy(bytes: &mut [u8]) -> StoreResult<()> {
    bytes.fill(0xa5);
    Ok(())
}

fn proof(challenge: Challenge, key: &SigningKey) -> Vec<u8> {
    canonical_json(&Proof::signed(challenge, key, 1000).unwrap()).unwrap()
}

#[test]
fn pending_challenge_and_possession_survive_reopen_without_authority() {
    let root = root();
    let (trust, key, request) = fixture();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let proposal = CandidateProposal {
        principal: request.change.actor,
        device: request.device,
        credential: request.credential,
        public_key: key.verifying_key().to_bytes(),
        lifetime_secs: 300,
    };
    let issued = store
        .issue_candidate(&request.change.operation, &proposal)
        .unwrap();
    let proof = Proof::signed(issued.clone(), &key, issued.issued_at).unwrap();
    let bytes = canonical_json(&proof).unwrap();
    let observed = store.observe_candidate(&bytes).unwrap();
    assert!(observed.proof.is_some());
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
    drop(store);
    let mut store = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    let repeated = store
        .issue_candidate(&request.change.operation, &proposal)
        .unwrap();
    assert_eq!(
        canonical_json(&repeated).unwrap(),
        canonical_json(&issued).unwrap()
    );
    let repeated = store.observe_candidate(&bytes).unwrap();
    assert_eq!(repeated.observed_at, observed.observed_at);
    let stored = store
        .candidate(&OperationId::parse(&issued.operation_id).unwrap())
        .unwrap();
    assert_eq!(canonical_json(&stored.proof.unwrap()).unwrap(), bytes);
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn pending_unknown_identity_does_not_authorize_signed_changes() {
    let root = root();
    let (trust, key, proposal, request_id) = proposal();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    let issued = store
        .issue_candidate_using(&request_id, &proposal, now, fixed_entropy)
        .unwrap();
    store
        .observe_candidate_using(&proof(issued, &key), now)
        .unwrap();
    let (_, _, mut request) = fixture();
    request.change.actor = proposal.principal.clone();
    request.device = proposal.device.clone();
    request.credential = proposal.credential.clone();
    let signed = crate::team_authority::signed::SignedRequest {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    };
    assert_eq!(store.admit(&signed), Err(StoreError::Refused));
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn exact_retry_uses_original_challenge_without_entropy_and_rejects_mutation() {
    let root = root();
    let (trust, _, proposal, request) = proposal();
    let mut store = AuthorityStore::create_new(fd(&root), trust).unwrap();
    let issued = store
        .issue_candidate_using(&request, &proposal, now, fixed_entropy)
        .unwrap();
    let repeated = store
        .issue_candidate_using(&request, &proposal, later, unavailable_entropy)
        .unwrap();
    assert_eq!(
        canonical_json(&issued).unwrap(),
        canonical_json(&repeated).unwrap()
    );
    for field in 0..5 {
        let mut changed = proposal.clone();
        match field {
            0 => changed.principal = PrincipalId::parse("00000000000000000000000030").unwrap(),
            1 => changed.device = DeviceId::parse("00000000000000000000000031").unwrap(),
            2 => changed.credential = CredentialId::parse("00000000000000000000000032").unwrap(),
            3 => {
                changed.public_key = SigningKey::from_bytes(&[0x42; 32])
                    .verifying_key()
                    .to_bytes()
            }
            _ => changed.lifetime_secs = 299,
        }
        assert!(matches!(
            store.issue_candidate_using(&request, &changed, now, fixed_entropy),
            Err(StoreError::Refused)
        ));
    }
    assert_eq!(records(&store, 0).unwrap().len(), 1);
}

#[test]
fn expiry_and_backwards_clock_refuse_new_observation_but_keep_historical_evidence() {
    let root = root();
    let (trust, key, proposal, request) = proposal();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let issued = store
        .issue_candidate_using(&request, &proposal, now, fixed_entropy)
        .unwrap();
    let operation = OperationId::parse(&issued.operation_id).unwrap();
    let bytes = proof(issued, &key);
    assert!(matches!(
        store.observe_candidate_using(&bytes, before),
        Err(StoreError::Refused)
    ));
    store.observe_candidate_using(&bytes, later).unwrap();
    assert!(matches!(
        store.observe_candidate_using(&bytes, expired),
        Err(StoreError::Refused)
    ));
    assert!(matches!(
        store.issue_candidate_using(&request, &proposal, expired, fixed_entropy),
        Err(StoreError::Refused)
    ));
    drop(store);
    let mut store = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    let historical = store.candidate(&operation).unwrap();
    assert_eq!(historical.observed_at, Some(1299));
    assert!(historical.proof.is_some());
}

#[test]
fn new_authority_revision_stales_challenge_even_through_another_handle() {
    let root = root();
    let (trust, key, proposal, request_id) = proposal();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut first = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let mut second = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    let issued = first
        .issue_candidate_using(&request_id, &proposal, now, fixed_entropy)
        .unwrap();
    let (_, _, request) = fixture();
    let signed = crate::team_authority::signed::SignedRequest {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    };
    second.admit(&signed).unwrap();
    assert!(matches!(
        first.observe_candidate_using(&proof(issued.clone(), &key), now),
        Err(StoreError::Refused)
    ));
    assert!(matches!(
        first.issue_candidate_using(&request_id, &proposal, now, fixed_entropy),
        Err(StoreError::Refused)
    ));
    assert_eq!(first.expected.revision, 1);
    assert!(first
        .candidate(&OperationId::parse(&issued.operation_id).unwrap())
        .unwrap()
        .proof
        .is_none());
}

#[test]
fn proof_requires_exact_stored_challenge_and_strict_labels_signature_and_domain() {
    let root = root();
    let (trust, key, proposal, request) = proposal();
    let mut store = AuthorityStore::create_new(fd(&root), trust).unwrap();
    let issued = store
        .issue_candidate_using(&request, &proposal, now, fixed_entropy)
        .unwrap();
    for field in 0..11 {
        let mut changed = issued.clone();
        match field {
            0 => changed.organization_id = "00000000000000000000000030".into(),
            1 => changed.principal_id = "00000000000000000000000030".into(),
            2 => changed.device_id = "00000000000000000000000030".into(),
            3 => changed.credential_id = "00000000000000000000000030".into(),
            4 => changed.operation_id = "00000000000000000000000030".into(),
            5 => changed.base_revision = 1,
            6 => changed.nonce = "ff".repeat(32),
            7 => {
                changed.issued_at = 999;
                changed.expires_at = 1299;
            }
            8 => changed.expires_at = 1299,
            9 => changed.version = 2,
            _ => {
                changed.public_key = hex(SigningKey::from_bytes(&[0x42; 32])
                    .verifying_key()
                    .as_bytes())
            }
        }
        let mut altered = Proof::signed(issued.clone(), &key, 1000).unwrap();
        if field < 9 {
            altered = Proof::signed(changed, &key, 1000).unwrap();
        } else {
            altered.challenge = changed;
        }
        assert!(store
            .observe_candidate_using(&canonical_json(&altered).unwrap(), now)
            .is_err());
    }
    for field in 0..5 {
        let mut altered = Proof::signed(issued.clone(), &key, 1000).unwrap();
        match field {
            0 => altered.status = "approved".into(),
            1 => altered.authority = "approved".into(),
            2 => altered.challenge_use = "consumed".into(),
            3 => altered.signature = "00".repeat(64),
            _ => altered.signature = hex(&key.sign(b"vivac.team.change.v1\0").to_bytes()),
        }
        assert!(store
            .observe_candidate_using(&canonical_json(&altered).unwrap(), now)
            .is_err());
    }
    let pretty = serde_json::to_vec_pretty(&Proof::signed(issued, &key, 1000).unwrap()).unwrap();
    store.observe_candidate_using(&pretty, now).unwrap();
    let mut extra: serde_json::Value = serde_json::from_slice(&pretty).unwrap();
    extra["unreviewed"] = true.into();
    assert!(store
        .observe_candidate_using(&serde_json::to_vec(&extra).unwrap(), now)
        .is_err());
    assert!(store
        .observe_candidate_using(&vec![b' '; MAX_JSON + 1], now)
        .is_err());
    assert!(store.observe_candidate_using(b"{}", now).is_err());
}

#[test]
fn clock_entropy_invalid_window_and_collision_fail_without_publishing() {
    let root = root();
    let (trust, _, proposal, request) = proposal();
    let mut store = AuthorityStore::create_new(fd(&root), trust).unwrap();
    assert!(matches!(
        store.issue_candidate_using(&request, &proposal, unavailable_time, fixed_entropy),
        Err(StoreError::Unavailable)
    ));
    assert!(matches!(
        store.issue_candidate_using(&request, &proposal, now, unavailable_entropy),
        Err(StoreError::Unavailable)
    ));
    for lifetime_secs in [0, 301, u64::MAX] {
        let changed = CandidateProposal {
            lifetime_secs,
            ..proposal.clone()
        };
        assert!(store
            .issue_candidate_using(&request, &changed, now, fixed_entropy)
            .is_err());
    }
    assert!(store
        .issue_candidate_using(&request, &proposal, || Ok(i64::MAX as u64), fixed_entropy)
        .is_err());
    let weak = CandidateProposal {
        public_key: [0; 32],
        ..proposal.clone()
    };
    assert!(store
        .issue_candidate_using(&request, &weak, now, fixed_entropy)
        .is_err());
    assert_eq!(records(&store, 0).unwrap().len(), 0);
    store
        .issue_candidate_using(&request, &proposal, now, fixed_entropy)
        .unwrap();
    let another = OperationId::parse("00000000000000000000000040").unwrap();
    assert!(store
        .issue_candidate_using(&another, &proposal, now, fixed_entropy)
        .is_err());
    assert_eq!(records(&store, 0).unwrap().len(), 1);
}

#[test]
fn capacity_keeps_expired_records_and_allows_exact_retry_when_full() {
    let root = root();
    let (trust, key, proposal, _) = proposal();
    let mut store = AuthorityStore::create_new(fd(&root), trust).unwrap();
    let mut first = None;
    for index in 0..MAX_PENDING {
        let request = OperationId::parse(&format!("{:026}", index + 100)).unwrap();
        let issued = store
            .issue_candidate_using(&request, &proposal, now, system_entropy)
            .unwrap();
        if index == 0 {
            first = Some((request, issued));
        }
    }
    let (request, issued) = first.unwrap();
    store
        .issue_candidate_using(&request, &proposal, later, unavailable_entropy)
        .unwrap();
    store
        .observe_candidate_using(&proof(issued, &key), later)
        .unwrap();
    let another = OperationId::parse("00000000000000000000000999").unwrap();
    assert!(store
        .issue_candidate_using(&another, &proposal, expired, system_entropy)
        .is_err());
    assert_eq!(records(&store, 0).unwrap().len(), MAX_PENDING);
    assert_eq!(store.checkpoint().unwrap().revision, 0);
}

#[test]
fn pending_mutations_rollback_or_poison_at_each_commit_boundary() {
    for observe in [false, true] {
        for point in [
            FaultPoint::AfterInsert,
            FaultPoint::BeforeCommit,
            FaultPoint::AfterCommit,
        ] {
            let root = root();
            let (trust, key, proposal, request) = proposal();
            let checkpoint = trust.bootstrap_checkpoint().unwrap();
            let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
            let bytes = if observe {
                let issued = store
                    .issue_candidate_using(&request, &proposal, now, fixed_entropy)
                    .unwrap();
                Some(proof(issued, &key))
            } else {
                None
            };
            let hook: fn(FaultPoint) -> bool = match point {
                FaultPoint::AfterInsert => |p| p == FaultPoint::AfterInsert,
                FaultPoint::BeforeCommit => |p| p == FaultPoint::BeforeCommit,
                FaultPoint::AfterCommit => |p| p == FaultPoint::AfterCommit,
            };
            store.set_fault_hook(Some(hook));
            let result = if let Some(bytes) = bytes {
                store.observe_candidate_using(&bytes, now).map(|_| ())
            } else {
                store
                    .issue_candidate_using(&request, &proposal, now, fixed_entropy)
                    .map(|_| ())
            };
            assert_eq!(
                result,
                Err(if point == FaultPoint::AfterCommit {
                    StoreError::CommitUnknown
                } else {
                    StoreError::Unpublished
                })
            );
            if point == FaultPoint::AfterCommit {
                assert_eq!(store.checkpoint(), Err(StoreError::CommitUnknown));
            }
            drop(store);
            let mut reopened = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
            let rows = records(&reopened, 0).unwrap();
            assert_eq!(
                rows.len(),
                usize::from(observe || point == FaultPoint::AfterCommit)
            );
            if observe {
                assert_eq!(
                    rows[0].observation.proof.is_some(),
                    point == FaultPoint::AfterCommit
                );
            }
            assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
        }
    }
}

#[test]
fn pending_busy_commit_reopens_without_persisted_candidate() {
    let root = root();
    let (trust, _, proposal, request) = proposal();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut writer = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let reader = AuthorityStore::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    reader.database.execute("BEGIN").unwrap();
    let mut held = reader
        .database
        .prepare("SELECT revision FROM meta")
        .unwrap();
    assert!(held.row().unwrap());
    assert_eq!(held.integer(0).unwrap(), checkpoint.revision);
    assert_eq!(
        writer
            .issue_candidate_using(&request, &proposal, now, fixed_entropy)
            .map(|_| ()),
        Err(StoreError::CommitUnknown)
    );
    assert_eq!(writer.checkpoint(), Err(StoreError::CommitUnknown));
    drop(writer);
    drop(held);
    reader.database.execute("ROLLBACK").unwrap();
    drop(reader);

    let mut reopened = AuthorityStore::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    assert!(records(&reopened, checkpoint.revision).unwrap().is_empty());
    assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
    assert!(matches!(
        reopened.issue_candidate_using(&request, &proposal, now, unavailable_entropy),
        Err(StoreError::Unavailable)
    ));
    assert!(records(&reopened, checkpoint.revision).unwrap().is_empty());
    assert_eq!(reopened.checkpoint().unwrap(), checkpoint);

    let issued = reopened
        .issue_candidate_using(&request, &proposal, now, fixed_entropy)
        .unwrap();
    assert_eq!(records(&reopened, checkpoint.revision).unwrap().len(), 1);
    assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
    drop(reopened);
    let mut reopened = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    let repeated = reopened
        .issue_candidate_using(&request, &proposal, now, unavailable_entropy)
        .unwrap();
    assert_eq!(
        canonical_json(&issued).unwrap(),
        canonical_json(&repeated).unwrap()
    );
    assert_eq!(records(&reopened, checkpoint.revision).unwrap().len(), 1);
    assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
}

#[test]
fn existing_writer_blocks_pending_issuance_without_partial_records() {
    let root = root();
    let (trust, _, proposal, request) = proposal();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut first = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let second = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    second.database.execute("BEGIN IMMEDIATE").unwrap();
    assert!(matches!(
        first.issue_candidate_using(&request, &proposal, now, fixed_entropy),
        Err(StoreError::Unpublished)
    ));
    second.database.execute("ROLLBACK").unwrap();
    assert!(records(&first, 0).unwrap().is_empty());
    first
        .issue_candidate_using(&request, &proposal, now, fixed_entropy)
        .unwrap();
}

#[test]
fn malformed_pending_rows_and_future_revision_refuse_reopen() {
    for field in 0..5 {
        let root = root();
        let (trust, key, proposal, request) = proposal();
        let checkpoint = trust.bootstrap_checkpoint().unwrap();
        let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
        let mut issued = store
            .issue_candidate_using(&request, &proposal, now, fixed_entropy)
            .unwrap();
        match field {
            0 => issued.organization_id = "00000000000000000000000030".into(),
            1 => issued.base_revision = 1,
            2 => issued.nonce = "ff".repeat(32),
            _ => {}
        }
        if field == 4 {
            let mut altered = Proof::signed(issued.clone(), &key, 1000).unwrap();
            altered.signature = "00".repeat(64);
            let mut update = store
                .database
                .prepare("UPDATE pending SET proof=?1,observed_at=1000")
                .unwrap();
            update
                .bind_blob(1, &canonical_json(&altered).unwrap())
                .unwrap();
            update.done().unwrap();
        } else {
            let bytes = if field == 3 {
                serde_json::to_vec_pretty(&issued).unwrap()
            } else {
                canonical_json(&issued).unwrap()
            };
            let mut update = store
                .database
                .prepare("UPDATE pending SET challenge=?1")
                .unwrap();
            update.bind_blob(1, &bytes).unwrap();
            update.done().unwrap();
        }
        drop(store);
        assert!(AuthorityStore::open_existing(fd(&root), trust, checkpoint).is_err());
    }
}

#[test]
fn legacy_schema_is_refused_without_migration() {
    let root = root();
    let (trust, _, _, _) = proposal();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    store.database.execute("DROP TABLE pending").unwrap();
    store.database.execute("DROP TABLE meta").unwrap();
    store
        .database
        .execute(&super::super::METADATA_SCHEMA.replace("version=2", "version=1"))
        .unwrap();
    let mut insert = store
        .database
        .prepare("INSERT INTO meta VALUES(1,1,?1,?2,?3,0,?3)")
        .unwrap();
    insert
        .bind_blob(1, trust.organization.as_str().as_bytes())
        .unwrap();
    insert
        .bind_blob(2, trust.initial_principal.as_str().as_bytes())
        .unwrap();
    insert.bind_blob(3, &checkpoint.digest).unwrap();
    insert.done().unwrap();
    drop(insert);
    drop(store);
    assert!(matches!(
        AuthorityStore::open_existing(fd(&root), trust, checkpoint),
        Err(StoreError::Refused)
    ));
}

#[test]
fn pending_crash_child() {
    let Some(path) = std::env::var_os("VIVAC_PENDING_CRASH_ROOT") else {
        return;
    };
    let (trust, key, proposal, request) = proposal();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut store =
        AuthorityStore::open_existing(std::fs::File::open(path).unwrap().into(), trust, checkpoint)
            .unwrap();
    let observe = std::env::var("VIVAC_PENDING_CRASH_ACTION").unwrap() == "observe";
    let bytes = if observe {
        Some(proof(
            records(&store, 0).unwrap()[0].observation.challenge.clone(),
            &key,
        ))
    } else {
        None
    };
    store.set_fault_hook(Some(|point| {
        let target = match std::env::var("VIVAC_PENDING_CRASH_POINT").unwrap().as_str() {
            "before" => FaultPoint::BeforeCommit,
            "after" => FaultPoint::AfterCommit,
            _ => panic!("Invalid pending crash fixture point"),
        };
        if point == target {
            unsafe { libc::_exit(77) };
        }
        false
    }));
    if let Some(bytes) = bytes {
        let _ = store.observe_candidate_using(&bytes, now);
    } else {
        let _ = store.issue_candidate_using(&request, &proposal, now, fixed_entropy);
    }
    panic!("Pending crash fixture did not reach the commit boundary");
}

#[test]
fn pending_process_exit_at_commit_reconciles_issuance_and_observation() {
    for action in ["issue", "observe"] {
        for point in ["before", "after"] {
            let root = root();
            let (trust, _, proposal, request) = proposal();
            let checkpoint = trust.bootstrap_checkpoint().unwrap();
            let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
            if action == "observe" {
                store
                    .issue_candidate_using(&request, &proposal, now, fixed_entropy)
                    .unwrap();
            }
            drop(store);
            let module = module_path!().split_once("::").unwrap().1;
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--quiet",
                    "--exact",
                    &format!("{module}::pending_crash_child"),
                ])
                .env("VIVAC_PENDING_CRASH_ROOT", root.path())
                .env("VIVAC_PENDING_CRASH_POINT", point)
                .env("VIVAC_PENDING_CRASH_ACTION", action)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(77));
            let mut reopened = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
            let rows = records(&reopened, 0).unwrap();
            assert_eq!(
                rows.len(),
                usize::from(action == "observe" || point == "after")
            );
            if action == "observe" {
                assert_eq!(rows[0].observation.proof.is_some(), point == "after");
            }
            reopened
                .issue_candidate_using(&request, &proposal, now, fixed_entropy)
                .unwrap();
            assert_eq!(records(&reopened, 0).unwrap().len(), 1);
            assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
        }
    }
}

fn wait_for(path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "Pending race fixture timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn pending_race_child() {
    let Some(path) = std::env::var_os("VIVAC_PENDING_RACE_ROOT") else {
        return;
    };
    let (trust, _, proposal, request) = proposal();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut store =
        AuthorityStore::open_existing(std::fs::File::open(path).unwrap().into(), trust, checkpoint)
            .unwrap();
    std::fs::write(std::env::var_os("VIVAC_PENDING_RACE_READY").unwrap(), []).unwrap();
    wait_for(std::path::Path::new(
        &std::env::var_os("VIVAC_PENDING_RACE_GO").unwrap(),
    ));
    match store.issue_candidate_using(&request, &proposal, now, fixed_entropy) {
        Ok(_) => {}
        Err(StoreError::Unpublished) => std::process::exit(75),
        Err(StoreError::CommitUnknown) => {
            assert_eq!(store.checkpoint(), Err(StoreError::CommitUnknown));
            std::process::exit(76);
        }
        Err(error) => panic!("Unexpected pending race result: {error:?}"),
    }
}

struct PendingChild(std::process::Child);

impl Drop for PendingChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn racing_processes_publish_one_idempotent_pending_challenge() {
    let root = root();
    let rendezvous = super::super::tests::root();
    let (trust, _, proposal, request) = proposal();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    drop(AuthorityStore::create_new(fd(&root), trust.clone()).unwrap());
    let module = module_path!().split_once("::").unwrap().1;
    let go = rendezvous.path().join("go");
    let mut children = Vec::new();
    for index in 0..2 {
        let ready = rendezvous.path().join(format!("ready-{index}"));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--quiet",
                "--exact",
                &format!("{module}::pending_race_child"),
            ])
            .env("VIVAC_PENDING_RACE_ROOT", root.path())
            .env("VIVAC_PENDING_RACE_READY", &ready)
            .env("VIVAC_PENDING_RACE_GO", &go)
            .spawn()
            .unwrap();
        children.push(PendingChild(child));
        wait_for(&ready);
    }
    std::fs::write(go, []).unwrap();
    let statuses: Vec<_> = children
        .iter_mut()
        .map(|child| child.0.wait().unwrap().code().unwrap())
        .collect();
    assert!(
        statuses.iter().any(|status| matches!(status, 0 | 76)),
        "Unexpected pending writer statuses: {statuses:?}"
    );
    assert!(
        statuses.iter().all(|status| matches!(status, 0 | 75 | 76)),
        "Unexpected pending writer statuses: {statuses:?}"
    );
    // Reconcile only after both writers exit. A successful acknowledgment
    // requires a row; an uncertain commit can leave either persistence outcome.
    let mut reopened = AuthorityStore::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    let rows = records(&reopened, 0).unwrap();
    assert!(rows.len() <= 1, "Pending writer statuses: {statuses:?}");
    if statuses.contains(&0) {
        assert_eq!(rows.len(), 1, "Pending writer statuses: {statuses:?}");
    }
    if let Some(row) = rows.first() {
        let issued = reopened
            .issue_candidate_using(&request, &proposal, now, unavailable_entropy)
            .unwrap();
        assert_eq!(
            canonical_json(&issued).unwrap(),
            canonical_json(&row.observation.challenge).unwrap()
        );
    } else {
        assert!(
            !statuses.contains(&0),
            "Pending writer statuses: {statuses:?}"
        );
        assert!(matches!(
            reopened.issue_candidate_using(&request, &proposal, now, unavailable_entropy),
            Err(StoreError::Unavailable)
        ));
        assert!(records(&reopened, 0).unwrap().is_empty());
        assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
        let issued = reopened
            .issue_candidate_using(&request, &proposal, now, fixed_entropy)
            .unwrap();
        assert_eq!(records(&reopened, 0).unwrap().len(), 1);
        assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
        drop(reopened);
        reopened = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
        let repeated = reopened
            .issue_candidate_using(&request, &proposal, now, unavailable_entropy)
            .unwrap();
        assert_eq!(
            canonical_json(&issued).unwrap(),
            canonical_json(&repeated).unwrap()
        );
    }
    assert_eq!(records(&reopened, 0).unwrap().len(), 1);
    assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
}
