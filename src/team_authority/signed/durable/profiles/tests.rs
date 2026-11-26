use std::fs;
use std::os::unix::fs::PermissionsExt;

use ed25519_dalek::{Signer, SigningKey};

use super::super::tests::{fd, fixture, root};
use super::super::FixtureTrustV2;
use super::*;
use crate::team_authority::signed::v2::V2Action;
use crate::team_authority::OperationId;

fn fixture_v2() -> (FixtureTrustV2, SigningKey, DeviceRequestV2) {
    let (base, key, request) = fixture();
    let principal = base.initial_principal.clone();
    (
        FixtureTrustV2 {
            base,
            policy_admin: principal.clone(),
        },
        key,
        DeviceRequestV2 {
            organization: request.change.organization,
            actor: request.change.actor,
            device: request.device,
            credential: request.credential,
            operation: request.change.operation,
            base_revision: 0,
            action: V2Action::SetOwnAdditionalDevice {
                principal,
                enabled: true,
            },
        },
    )
}

fn sign_v2(request: DeviceRequestV2, key: &SigningKey) -> SignedRequestV2 {
    SignedRequestV2 {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    }
}

fn other_fixture_key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 2: published fixture seed.
    let seed = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";
    let bytes = std::array::from_fn(|index| {
        u8::from_str_radix(&seed[index * 2..index * 2 + 2], 16).unwrap()
    });
    SigningKey::from_bytes(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn own_device(store: &mut AuthorityStoreV2, trust: &FixtureTrustV2) -> bool {
    store
        .can_add_own_device(&trust.base.organization, &trust.base.initial_principal)
        .unwrap()
}

fn next_request(request: &DeviceRequestV2, revision: u64) -> DeviceRequestV2 {
    let mut next = request.clone();
    next.base_revision = revision;
    next.operation = OperationId::parse(&format!("{:026}", revision + 5)).unwrap();
    next
}

#[test]
fn v2_bootstrap_and_chain_match_independent_vector() {
    let root = root();
    let (trust, key, request) = fixture_v2();
    assert_eq!(
        hex(&trust.base.bootstrap_checkpoint().unwrap().digest),
        "c179092944e28a5b630d7c72071d18136eeb5364c8f1070c3c435441114d5944"
    );
    let bootstrap = trust.bootstrap_checkpoint().unwrap();
    assert_eq!(bootstrap.revision, 0);
    assert_eq!(
        hex(&bootstrap.digest),
        "86b864cec0de1b62bcafc3b28878f928e4ce8e9dfa08022f567987b76eef4c90"
    );
    let signed = sign_v2(request, &key);
    assert_eq!(
        hex(&Sha256::digest(signed.request.canonical_bytes())),
        "0810cacfff090a6600411362968236abeb1e40b7800e957eac630048b2dc443d"
    );
    assert_eq!(hex(&signed.signature),
        "55b28c3cff81dbdfa853821accbb880021df8c279e178aedb59fe7f58cc35368a5ca2c199ee8b321089b9a50d36d15534f8a48f72fd8f2681a27e492aabb4e0f");
    let mut store = AuthorityStoreV2::create_new(fd(&root), trust).unwrap();
    assert_eq!(store.admit(&signed).unwrap().revision, 1);
    let checkpoint = store.checkpoint().unwrap();
    assert_eq!(checkpoint.revision, 1);
    assert_eq!(
        hex(&checkpoint.digest),
        "71487c1e190014ff39708e58c6c9d6e72342e62aaef58765ffe8a71f6477d497"
    );
}

#[test]
fn v2_stale_handles_refresh_queries_and_refuse_stale_admission() {
    let root = root();
    let (trust, key, request) = fixture_v2();
    let bootstrap = trust.bootstrap_checkpoint().unwrap();
    let mut first = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
    let mut second = AuthorityStoreV2::open_existing(fd(&root), trust.clone(), bootstrap).unwrap();
    assert!(!own_device(&mut second, &trust));
    let signed = sign_v2(request.clone(), &key);
    first.admit(&signed).unwrap();
    assert!(own_device(&mut second, &trust));
    assert_eq!(
        second.admit(&signed).unwrap().admission,
        Admission::Historical
    );
    let mut stale = request.clone();
    stale.operation = OperationId::parse("00000000000000000000000009").unwrap();
    assert_eq!(
        second.admit(&sign_v2(stale, &key)),
        Err(StoreError::Refused)
    );
    let mut revoke = next_request(&request, 1);
    revoke.action = V2Action::SetOwnAdditionalDevice {
        principal: trust.base.initial_principal.clone(),
        enabled: false,
    };
    first.admit(&sign_v2(revoke, &key)).unwrap();
    assert!(!own_device(&mut second, &trust));
    let mut revoke_admin = next_request(&request, 2);
    revoke_admin.action = V2Action::SetEnrollmentPolicyAdministrator {
        principal: trust.policy_admin.clone(),
        enabled: false,
    };
    first.admit(&sign_v2(revoke_admin, &key)).unwrap();
    assert!(!second
        .can_manage_enrollment_policy(&trust.base.organization, &trust.policy_admin)
        .unwrap());
    assert_eq!(second.checkpoint().unwrap(), first.checkpoint().unwrap());
}

#[test]
fn v2_binding_signature_counter_and_operation_refusals_preserve_history() {
    for changed in [
        "organization",
        "actor",
        "device",
        "credential",
        "signature",
        "key",
        "domain",
        "counter",
    ] {
        let root = root();
        let (trust, key, request) = fixture_v2();
        let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
        let checkpoint = store.checkpoint().unwrap();
        let mut invalid = request.clone();
        match changed {
            "organization" => {
                invalid.organization = OrganizationId::parse("00000000000000000000000009").unwrap()
            }
            "actor" => invalid.actor = PrincipalId::parse("00000000000000000000000009").unwrap(),
            "device" => {
                invalid.device =
                    crate::team_authority::DeviceId::parse("00000000000000000000000009").unwrap()
            }
            "credential" => {
                invalid.credential =
                    crate::team_authority::CredentialId::parse("00000000000000000000000009")
                        .unwrap()
            }
            "counter" => invalid.base_revision = 1,
            "signature" | "key" | "domain" => {}
            _ => unreachable!(),
        }
        let mut invalid = sign_v2(invalid, &key);
        if changed == "signature" {
            invalid.signature[0] ^= 1;
        }
        if changed == "key" {
            invalid = sign_v2(invalid.request, &other_fixture_key());
        }
        if changed == "domain" {
            let bytes = invalid.request.canonical_bytes();
            let mut other_domain = b"vivac.team.change.v1\0".to_vec();
            other_domain.extend_from_slice(&bytes[b"vivac.team.change.v2\0".len()..]);
            invalid.signature = key.sign(&other_domain).to_bytes();
        }
        assert_eq!(store.admit(&invalid), Err(StoreError::Refused), "{changed}");
        assert_eq!(store.checkpoint().unwrap(), checkpoint);
        let signed = sign_v2(request.clone(), &key);
        store.admit(&signed).unwrap();
        let admitted = store.checkpoint().unwrap();
        let mut changed_operation = request;
        changed_operation.action = V2Action::SetOwnAdditionalDevice {
            principal: trust.policy_admin,
            enabled: false,
        };
        assert_eq!(
            store.admit(&sign_v2(changed_operation, &key)),
            Err(StoreError::Refused)
        );
        assert_eq!(store.checkpoint().unwrap(), admitted);
    }
}

#[test]
fn v2_revoked_actor_and_removed_policy_role_cannot_grant() {
    for revoke_principal in [false, true] {
        let root = root();
        let (trust, key, request) = fixture_v2();
        let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
        let grant = sign_v2(request.clone(), &key);
        store.admit(&grant).unwrap();
        let mut revoke = next_request(&request, 1);
        revoke.action = if revoke_principal {
            V2Action::Classic(crate::team_authority::Action::RevokePrincipal {
                principal: trust.policy_admin.clone(),
            })
        } else {
            V2Action::SetEnrollmentPolicyAdministrator {
                principal: trust.policy_admin.clone(),
                enabled: false,
            }
        };
        store.admit(&sign_v2(revoke, &key)).unwrap();
        let checkpoint = store.checkpoint().unwrap();
        drop(store);
        let mut store =
            AuthorityStoreV2::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
        assert!(!store
            .can_manage_enrollment_policy(&trust.base.organization, &trust.policy_admin)
            .unwrap());
        assert_eq!(own_device(&mut store, &trust), !revoke_principal);
        assert_eq!(
            store.admit(&sign_v2(next_request(&request, 2), &key)),
            Err(StoreError::Refused)
        );
        if revoke_principal {
            assert_eq!(store.admit(&grant), Err(StoreError::Refused));
        }
        assert_eq!(store.checkpoint().unwrap(), checkpoint);
    }
}

#[test]
fn v2_transaction_faults_roll_back_or_reconcile_poisoned_handles() {
    for point in [
        FaultPoint::AfterInsert,
        FaultPoint::BeforeCommit,
        FaultPoint::AfterCommit,
    ] {
        let root = root();
        let (trust, key, request) = fixture_v2();
        let bootstrap = trust.bootstrap_checkpoint().unwrap();
        let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
        let hook: fn(FaultPoint) -> bool = match point {
            FaultPoint::AfterInsert => |current| current == FaultPoint::AfterInsert,
            FaultPoint::BeforeCommit => |current| current == FaultPoint::BeforeCommit,
            FaultPoint::AfterCommit => |current| current == FaultPoint::AfterCommit,
        };
        store.set_fault_hook(Some(hook));
        let signed = sign_v2(request, &key);
        if point == FaultPoint::AfterCommit {
            assert_eq!(store.admit(&signed), Err(StoreError::CommitUnknown));
            assert_eq!(store.checkpoint(), Err(StoreError::CommitUnknown));
            assert_eq!(
                store.can_add_own_device(&trust.base.organization, &trust.policy_admin),
                Err(StoreError::CommitUnknown)
            );
            assert_eq!(store.admit(&signed), Err(StoreError::CommitUnknown));
        } else {
            assert_eq!(store.admit(&signed), Err(StoreError::Unpublished));
            assert_eq!(store.checkpoint().unwrap(), bootstrap);
            assert!(!own_device(&mut store, &trust));
        }
        drop(store);
        let mut store =
            AuthorityStoreV2::open_existing(fd(&root), trust.clone(), bootstrap).unwrap();
        assert_eq!(
            own_device(&mut store, &trust),
            point == FaultPoint::AfterCommit
        );
        assert_eq!(
            store.checkpoint().unwrap().revision,
            u64::from(point == FaultPoint::AfterCommit)
        );
        assert_eq!(
            store.admit(&signed).unwrap().admission,
            if point == FaultPoint::AfterCommit {
                Admission::Historical
            } else {
                Admission::NewlyAdmitted
            }
        );
        assert!(own_device(&mut store, &trust));
    }
}

#[test]
fn profiles_refuse_cross_open_and_existing_creation_without_changing_bytes() {
    for version_two in [false, true] {
        let root = root();
        let (trust, _, _) = fixture_v2();
        if version_two {
            drop(AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap());
        } else {
            drop(AuthorityStore::create_new(fd(&root), trust.base.clone()).unwrap());
        }
        let database = root.path().join("authority.sqlite3");
        let before = fs::read(&database).unwrap();
        assert!(matches!(
            AuthorityStore::create_new(fd(&root), trust.base.clone()),
            Err(StoreError::Refused)
        ));
        assert!(matches!(
            AuthorityStoreV2::create_new(fd(&root), trust.clone()),
            Err(StoreError::Refused)
        ));
        if version_two {
            assert!(matches!(
                AuthorityStore::open_existing(
                    fd(&root),
                    trust.base.clone(),
                    trust.base.bootstrap_checkpoint().unwrap()
                ),
                Err(StoreError::Refused)
            ));
        } else {
            assert!(matches!(
                AuthorityStoreV2::open_existing(
                    fd(&root),
                    trust.clone(),
                    trust.bootstrap_checkpoint().unwrap()
                ),
                Err(StoreError::Refused)
            ));
        }
        assert_eq!(fs::read(database).unwrap(), before);
    }
}

#[test]
fn v2_rejects_changed_rows_domains_metadata_and_schema() {
    for tamper in [
        "UPDATE entries SET signature=zeroblob(64)",
        "UPDATE entries SET operation=CAST('00000000000000000000000009' AS BLOB)",
        "UPDATE entries SET seq=2",
        "UPDATE entries SET request=CAST(replace(CAST(request AS TEXT),'change.v2','change.v1') AS BLOB)",
        "UPDATE meta SET trust=zeroblob(32)",
        "UPDATE meta SET head=zeroblob(32)",
        "UPDATE meta SET organization=CAST('00000000000000000000000009' AS BLOB)",
        "CREATE TABLE unexpected (value BLOB) STRICT",
        pending::SCHEMA,
        "CREATE INDEX unexpected ON entries(seq)",
    ] {
        let root = root();
        let (trust, key, request) = fixture_v2();
        let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
        store.admit(&sign_v2(request, &key)).unwrap();
        let checkpoint = store.checkpoint().unwrap();
        store.database.execute(tamper).unwrap();
        assert_eq!(store.checkpoint(), Err(StoreError::Refused), "{tamper}");
        drop(store);
        assert!(matches!(AuthorityStoreV2::open_existing(fd(&root), trust, checkpoint), Err(StoreError::Refused)), "{tamper}");
    }
}

#[test]
fn v2_external_trust_and_checkpoint_conflicts_refuse_without_mutating_database() {
    let root = root();
    let (trust, key, request) = fixture_v2();
    let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
    store.admit(&sign_v2(request, &key)).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    drop(store);
    let database = root.path().join("authority.sqlite3");
    let before = fs::read(&database).unwrap();
    for change in [
        "admin",
        "key",
        "organization",
        "binding",
        "checkpoint",
        "future",
    ] {
        let mut altered = trust.clone();
        let mut expected = checkpoint;
        match change {
            "admin" => {
                altered.policy_admin = PrincipalId::parse("00000000000000000000000009").unwrap()
            }
            "key" => {
                let other = other_fixture_key();
                let (org, principal, device, credential, _) =
                    altered.base.known[0].binding_fields();
                altered.base.known[0] = KnownDevice::new(
                    org.clone(),
                    principal.clone(),
                    device.clone(),
                    credential.clone(),
                    other.verifying_key().to_bytes(),
                )
                .unwrap();
            }
            "organization" => {
                altered.base.organization =
                    OrganizationId::parse("00000000000000000000000009").unwrap()
            }
            "binding" => altered.base.known.clear(),
            "checkpoint" => expected.digest[0] ^= 1,
            "future" => expected.revision += 1,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                AuthorityStoreV2::open_existing(fd(&root), altered, expected),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
        assert_eq!(fs::read(&database).unwrap(), before);
    }
}

#[test]
fn v2_replacing_the_root_capability_refuses_live_access() {
    let root = root();
    let replacement = super::super::tests::root();
    let (trust, key, request) = fixture_v2();
    let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
    let replacement_fd = fd(&replacement);
    assert_eq!(
        unsafe {
            libc::dup3(
                replacement_fd.as_raw_fd(),
                store.root.as_raw_fd(),
                libc::O_CLOEXEC,
            )
        },
        store.root.as_raw_fd()
    );
    assert_eq!(store.checkpoint(), Err(StoreError::Refused));
    assert_eq!(
        store.admit(&sign_v2(request, &key)),
        Err(StoreError::Refused)
    );
    assert_eq!(
        store.can_manage_enrollment_policy(&trust.base.organization, &trust.policy_admin),
        Err(StoreError::Refused)
    );
}

#[test]
fn v2_invalid_bootstrap_and_public_root_never_create_a_database() {
    for change in ["admin", "duplicate_binding", "public_root"] {
        let root = root();
        let (mut trust, _, _) = fixture_v2();
        match change {
            "admin" => {
                trust.policy_admin = PrincipalId::parse("00000000000000000000000009").unwrap()
            }
            "duplicate_binding" => trust.base.known.push(trust.base.known[0].clone()),
            "public_root" => {
                fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap()
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                AuthorityStoreV2::create_new(fd(&root), trust),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
        assert!(!root.path().join("authority.sqlite3").exists());
    }
}

#[test]
fn v2_schema_has_only_authority_history_and_queries_keep_organization_scope() {
    let root = root();
    let (trust, key, request) = fixture_v2();
    let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
    let mut metadata = store.database.prepare("SELECT version FROM meta").unwrap();
    assert!(metadata.row().unwrap());
    assert_eq!(metadata.integer(0).unwrap(), 3);
    assert!(!metadata.row().unwrap());
    drop(metadata);
    let mut schema = store
        .database
        .prepare("SELECT name FROM sqlite_schema ORDER BY name")
        .unwrap();
    for name in [
        b"entries".as_slice(),
        b"meta".as_slice(),
        b"sqlite_autoindex_entries_1".as_slice(),
    ] {
        assert!(schema.row().unwrap());
        assert_eq!(schema.text(0, 64).unwrap(), name);
    }
    assert!(!schema.row().unwrap());
    drop(schema);
    store.admit(&sign_v2(request, &key)).unwrap();
    let other = OrganizationId::parse("00000000000000000000000009").unwrap();
    assert!(!store
        .can_add_own_device(&other, &trust.policy_admin)
        .unwrap());
    assert!(!store
        .can_manage_enrollment_policy(&other, &trust.policy_admin)
        .unwrap());
    assert!(own_device(&mut store, &trust));
}

#[test]
fn v2_operation_limit_preserves_exact_retry_at_full_history() {
    let root = root();
    let (trust, key, request) = fixture_v2();
    let store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
    let mut authority = V2Profile::bootstrap(&trust).unwrap();
    let mut checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut first = None;
    store.database.execute("BEGIN IMMEDIATE").unwrap();
    for revision in 0..MAX_OPERATIONS {
        let signed = sign_v2(next_request(&request, revision), &key);
        assert_eq!(
            authority
                .apply_signed(&signed, &trust.base.known[0])
                .unwrap(),
            Outcome {
                admission: Admission::NewlyAdmitted,
                revision: revision + 1
            }
        );
        if revision == 0 {
            first = Some(signed.clone());
        }
        let bytes = signed.request.canonical_bytes();
        checkpoint = advance(checkpoint, &bytes, &signed.signature);
        let mut insert = store
            .database
            .prepare("INSERT INTO entries VALUES(?1,?2,?3,?4)")
            .unwrap();
        insert.bind_integer(1, checkpoint.revision).unwrap();
        insert
            .bind_blob(2, signed.request.operation.as_str().as_bytes())
            .unwrap();
        insert.bind_blob(3, &bytes).unwrap();
        insert.bind_blob(4, &signed.signature).unwrap();
        insert.done().unwrap();
    }
    let mut metadata = store
        .database
        .prepare("UPDATE meta SET revision=?1,head=?2")
        .unwrap();
    metadata.bind_integer(1, checkpoint.revision).unwrap();
    metadata.bind_blob(2, &checkpoint.digest).unwrap();
    metadata.done().unwrap();
    drop(metadata);
    store.database.execute("COMMIT").unwrap();
    drop(store);
    let mut store = AuthorityStoreV2::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    assert!(own_device(&mut store, &trust));
    assert_eq!(
        store.admit(&sign_v2(next_request(&request, MAX_OPERATIONS), &key)),
        Err(StoreError::Refused)
    );
    assert_eq!(
        store.admit(&first.unwrap()).unwrap().admission,
        Admission::Historical
    );
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn v2_a_retained_checkpoint_refuses_rollback_to_an_old_database() {
    let root = root();
    let (trust, key, request) = fixture_v2();
    let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
    let database = root.path().join("authority.sqlite3");
    let old = fs::read(&database).unwrap();
    store.admit(&sign_v2(request, &key)).unwrap();
    let retained = store.checkpoint().unwrap();
    drop(store);
    fs::write(&database, &old).unwrap();
    assert!(matches!(
        AuthorityStoreV2::open_existing(fd(&root), trust, retained),
        Err(StoreError::Refused)
    ));
    assert_eq!(fs::read(database).unwrap(), old);
}

#[test]
fn v2_database_replacement_and_broad_permissions_refuse_live_queries() {
    for change in ["replacement", "root_permissions", "database_permissions"] {
        let root = root();
        let (trust, _, _) = fixture_v2();
        let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
        let database = root.path().join("authority.sqlite3");
        match change {
            "replacement" => {
                let bytes = fs::read(&database).unwrap();
                fs::rename(&database, root.path().join("old")).unwrap();
                fs::write(&database, bytes).unwrap();
                fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();
            }
            "root_permissions" => {
                fs::set_permissions(root.path(), fs::Permissions::from_mode(0o750)).unwrap()
            }
            "database_permissions" => {
                fs::set_permissions(&database, fs::Permissions::from_mode(0o640)).unwrap()
            }
            _ => unreachable!(),
        }
        assert_eq!(
            store.can_add_own_device(&trust.base.organization, &trust.policy_admin),
            Err(StoreError::Refused)
        );
        assert_eq!(store.checkpoint(), Err(StoreError::Refused));
    }
}

#[test]
fn v2_wal_header_namespace_and_file_limits_refuse_without_repair() {
    for change in ["header", "journal", "database", "wal", "shm"] {
        let root = root();
        let (trust, _, _) = fixture_v2();
        let checkpoint = trust.bootstrap_checkpoint().unwrap();
        drop(AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap());
        let database = root.path().join("authority.sqlite3");
        let mut bytes = fs::read(&database).unwrap();
        match change {
            "header" => {
                bytes[18] = 2;
                bytes[19] = 2;
                fs::write(&database, &bytes).unwrap();
            }
            "database" => fs::OpenOptions::new()
                .write(true)
                .open(&database)
                .unwrap()
                .set_len(MAX_DATABASE as u64 + 1)
                .unwrap(),
            _ => {
                let suffix = if change == "journal" {
                    "journal"
                } else {
                    change
                };
                let path = root.path().join(format!("authority.sqlite3-{suffix}"));
                let file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                if change == "journal" {
                    file.set_len(MAX_JOURNAL as u64 + 1).unwrap();
                }
            }
        }
        let before = fs::read(&database).unwrap();
        assert!(
            matches!(
                AuthorityStoreV2::open_existing(fd(&root), trust, checkpoint),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
        assert_eq!(fs::read(&database).unwrap(), before);
        if change == "header" {
            assert!(!root.path().join("authority.sqlite3-wal").exists());
            assert!(!root.path().join("authority.sqlite3-shm").exists());
        }
    }
}

#[test]
fn v2_policy_history_survives_reopen_and_revocation() {
    let root = root();
    let (base, key, classic) = fixture();
    let trust = FixtureTrustV2 {
        policy_admin: base.initial_principal.clone(),
        base,
    };
    let organization = trust.base.organization.clone();
    let principal = trust.base.initial_principal.clone();
    let mut store = AuthorityStoreV2::create_new(fd(&root), trust.clone()).unwrap();
    assert_eq!(
        store.checkpoint().unwrap(),
        trust.bootstrap_checkpoint().unwrap()
    );
    assert!(!store.can_add_own_device(&organization, &principal).unwrap());
    assert!(store
        .can_manage_enrollment_policy(&organization, &principal)
        .unwrap());
    let request = DeviceRequestV2 {
        organization: organization.clone(),
        actor: principal.clone(),
        device: classic.device,
        credential: classic.credential,
        operation: classic.change.operation,
        base_revision: 0,
        action: V2Action::SetOwnAdditionalDevice {
            principal: principal.clone(),
            enabled: true,
        },
    };
    let granted = SignedRequestV2 {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request: request.clone(),
    };
    assert_eq!(
        store
            .admit(&granted)
            .expect("the policy administrator can grant an own device"),
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 1
        }
    );
    assert!(store.can_add_own_device(&organization, &principal).unwrap());
    let checkpoint = store.checkpoint().unwrap();
    assert_eq!(checkpoint.revision, 1);
    drop(store);
    let mut store = AuthorityStoreV2::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    assert!(store.can_add_own_device(&organization, &principal).unwrap());
    assert_eq!(
        store.admit(&granted).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 1
        }
    );
    let mut revoke = request;
    revoke.operation = OperationId::parse("00000000000000000000000006").unwrap();
    revoke.base_revision = 1;
    revoke.action = V2Action::SetOwnAdditionalDevice {
        principal: principal.clone(),
        enabled: false,
    };
    let revoked = SignedRequestV2 {
        signature: key.sign(&revoke.canonical_bytes()).to_bytes(),
        request: revoke,
    };
    assert_eq!(
        store.admit(&revoked).unwrap(),
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 2
        }
    );
    assert!(!store.can_add_own_device(&organization, &principal).unwrap());
    let checkpoint = store.checkpoint().unwrap();
    assert_eq!(checkpoint.revision, 2);
    drop(store);
    let mut store = AuthorityStoreV2::open_existing(fd(&root), trust, checkpoint).unwrap();
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
    assert!(!store.can_add_own_device(&organization, &principal).unwrap());
    assert!(store
        .can_manage_enrollment_policy(&organization, &principal)
        .unwrap());
}
