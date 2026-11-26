use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;

use ed25519_dalek::{Signer, SigningKey};

use super::super::super::tests::{fd, fixture, root};
use super::*;
use crate::team_authority::signed::v2::V2Action;
use crate::team_authority::{Action, OperationId};

fn identifier(value: u64) -> String {
    format!("{value:026}")
}

fn other_key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 2: this is a published test seed.
    let seed = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";
    SigningKey::from_bytes(&std::array::from_fn(|index| {
        u8::from_str_radix(&seed[index * 2..index * 2 + 2], 16).unwrap()
    }))
}

fn fixture_registered() -> (FixtureRegisteredTrust, SigningKey, SigningKey) {
    let (mut base, key, _) = fixture();
    let other = other_key();
    base.known.push(
        KnownDevice::new(
            base.organization.clone(),
            base.initial_principal.clone(),
            DeviceId::parse(&identifier(6)).unwrap(),
            CredentialId::parse(&identifier(7)).unwrap(),
            other.verifying_key().to_bytes(),
        )
        .unwrap(),
    );
    let policy_admin = base.initial_principal.clone();
    (FixtureRegisteredTrust { base, policy_admin }, key, other)
}

fn policy(
    trust: &FixtureRegisteredTrust,
    key: &SigningKey,
    device: u64,
    credential: u64,
    operation: u64,
    revision: u64,
    enabled: bool,
) -> RegisteredRequest {
    let request = DeviceRequestV2 {
        organization: trust.base.organization.clone(),
        actor: trust.base.initial_principal.clone(),
        device: DeviceId::parse(&identifier(device)).unwrap(),
        credential: CredentialId::parse(&identifier(credential)).unwrap(),
        operation: OperationId::parse(&identifier(operation)).unwrap(),
        base_revision: revision,
        action: V2Action::SetOwnAdditionalDevice {
            principal: trust.base.initial_principal.clone(),
            enabled,
        },
    };
    RegisteredRequest::Policy(SignedRequestV2 {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    })
}

fn revoke(
    trust: &FixtureRegisteredTrust,
    key: &SigningKey,
    operation: u64,
    revision: u64,
) -> RegisteredRequest {
    let request = RevocationRequest {
        organization: trust.base.organization.clone(),
        actor: trust.base.initial_principal.clone(),
        device: DeviceId::parse(&identifier(3)).unwrap(),
        credential: CredentialId::parse(&identifier(4)).unwrap(),
        operation: OperationId::parse(&identifier(operation)).unwrap(),
        base_revision: revision,
        target_device: DeviceId::parse(&identifier(6)).unwrap(),
        target_credential: CredentialId::parse(&identifier(7)).unwrap(),
    };
    RegisteredRequest::Revoke(SignedRevocation {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    })
}

fn active(
    store: &mut AuthorityStoreRegistered,
    trust: &FixtureRegisteredTrust,
    device: u64,
    credential: u64,
) -> bool {
    store
        .binding_is_active(
            &trust.base.organization,
            &trust.base.initial_principal,
            &DeviceId::parse(&identifier(device)).unwrap(),
            &CredentialId::parse(&identifier(credential)).unwrap(),
        )
        .unwrap()
}

fn sign_policy(request: DeviceRequestV2, key: &SigningKey) -> RegisteredRequest {
    RegisteredRequest::Policy(SignedRequestV2 {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    })
}

fn sign_revocation(request: RevocationRequest, key: &SigningKey) -> RegisteredRequest {
    RegisteredRequest::Revoke(SignedRevocation {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    })
}

fn policy_request(request: RegisteredRequest) -> DeviceRequestV2 {
    match request {
        RegisteredRequest::Policy(signed) => signed.request,
        RegisteredRequest::Revoke(_) => unreachable!(),
    }
}

fn revocation_request(request: RegisteredRequest) -> RevocationRequest {
    match request {
        RegisteredRequest::Revoke(signed) => signed.request,
        RegisteredRequest::Policy(_) => unreachable!(),
    }
}

fn refuse(store: &mut AuthorityStoreRegistered, request: &RegisteredRequest) {
    let before = store.checkpoint().unwrap();
    assert_eq!(store.admit(request), Err(StoreError::Refused));
    assert_eq!(store.checkpoint().unwrap(), before);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn registered_revocation_survives_reopen_and_blocks_old_receipts() {
    let root = root();
    let (trust, key, other) = fixture_registered();
    let bootstrap = trust.bootstrap_checkpoint().unwrap();
    let old = policy(&trust, &other, 6, 7, 10, 0, true);
    let revoke = revoke(&trust, &key, 11, 1);
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    assert_eq!(store.admit(&old).unwrap().revision, 1);
    assert_eq!(store.admit(&revoke).unwrap().revision, 2);
    let retained = store.checkpoint().unwrap();
    drop(store);
    let mut store =
        AuthorityStoreRegistered::open_existing(fd(&root), trust.clone(), bootstrap).unwrap();
    assert_eq!(store.checkpoint().unwrap(), retained);
    assert!(!active(&mut store, &trust, 6, 7));
    assert!(active(&mut store, &trust, 3, 4));
    assert_eq!(store.admit(&old), Err(StoreError::Refused));
    assert_eq!(
        store.admit(&revoke).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 2
        }
    );
    assert_eq!(
        store
            .admit(&policy(&trust, &key, 3, 4, 12, 2, false))
            .unwrap()
            .revision,
        3
    );
}

#[test]
fn registered_bootstrap_and_mixed_chain_match_independent_vectors() {
    let root = root();
    let (trust, key, other) = fixture_registered();
    assert_eq!(
        hex(&trust.base.bootstrap_checkpoint().unwrap().digest),
        "369cf877934aba1b54d53ac11cc613d84838a20df616748a2cceea393f509bf8"
    );
    let bootstrap = trust.bootstrap_checkpoint().unwrap();
    assert_eq!(
        hex(&bootstrap.digest),
        "03d237d2b2f9c1c836bbfa334778e115772c8ce09c2f1784ce7d136462a3b61f"
    );
    let mut reordered = trust.clone();
    reordered.base.known.reverse();
    assert_eq!(reordered.bootstrap_checkpoint().unwrap(), bootstrap);
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    store
        .admit(&policy(&trust, &other, 6, 7, 5, 0, true))
        .unwrap();
    assert_eq!(
        hex(&store.checkpoint().unwrap().digest),
        "aa5369a10469e61bb35935b5d72ad530ac2a816f39aae894b1b814a4bcc29a4e"
    );
    store.admit(&revoke(&trust, &key, 8, 1)).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    assert_eq!(checkpoint.revision, 2);
    assert_eq!(
        hex(&checkpoint.digest),
        "5ca01603d9859648811c4ee2e53487a94e26931c71e10076fba8838e41da5384"
    );
    drop(store);
    let mut store =
        AuthorityStoreRegistered::open_existing(fd(&root), reordered, checkpoint).unwrap();
    assert!(!active(&mut store, &trust, 6, 7));
}

#[test]
fn registered_stale_handles_refresh_credential_queries_and_refuse_stale_writes() {
    let root = root();
    let (trust, key, other) = fixture_registered();
    let bootstrap = trust.bootstrap_checkpoint().unwrap();
    let mut first = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    let mut second =
        AuthorityStoreRegistered::open_existing(fd(&root), trust.clone(), bootstrap).unwrap();
    assert!(active(&mut second, &trust, 6, 7));
    let old = policy(&trust, &other, 6, 7, 5, 0, true);
    first.admit(&old).unwrap();
    first.admit(&revoke(&trust, &key, 8, 1)).unwrap();
    assert!(!active(&mut second, &trust, 6, 7));
    assert!(second
        .can_add_own_device(&trust.base.organization, &trust.policy_admin)
        .unwrap());
    refuse(&mut second, &old);
    refuse(&mut second, &policy(&trust, &key, 3, 4, 9, 1, false));
    assert_eq!(
        second
            .admit(&policy(&trust, &key, 3, 4, 9, 2, false))
            .unwrap()
            .revision,
        3
    );
    assert!(!first
        .can_add_own_device(&trust.base.organization, &trust.policy_admin)
        .unwrap());
}

#[test]
fn registered_revocation_receipts_are_exact_and_do_not_restore_a_binding() {
    let root = root();
    let (trust, key, _) = fixture_registered();
    let signed = revoke(&trust, &key, 5, 0);
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    store.admit(&signed).unwrap();
    store
        .admit(&policy(&trust, &key, 3, 4, 8, 1, true))
        .unwrap();
    let checkpoint = store.checkpoint().unwrap();
    assert_eq!(
        store.admit(&signed).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 1
        }
    );
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
    let mut changed = revocation_request(signed.clone());
    changed.base_revision = 2;
    refuse(&mut store, &sign_revocation(changed, &key));
    let mut changed = revocation_request(signed.clone());
    changed.target_device = DeviceId::parse(&identifier(3)).unwrap();
    changed.target_credential = CredentialId::parse(&identifier(4)).unwrap();
    refuse(&mut store, &sign_revocation(changed, &key));
    let mut changed = revocation_request(signed);
    changed.operation = OperationId::parse(&identifier(9)).unwrap();
    changed.base_revision = 2;
    refuse(&mut store, &sign_revocation(changed, &key));
    assert!(!active(&mut store, &trust, 6, 7));
}

#[test]
fn registered_self_revocation_leaves_no_automatic_recovery_after_restart() {
    let root = root();
    let (trust, key, other) = fixture_registered();
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    store.admit(&revoke(&trust, &key, 5, 0)).unwrap();
    let mut request = revocation_request(revoke(&trust, &key, 8, 1));
    request.target_device = request.device.clone();
    request.target_credential = request.credential.clone();
    let own = sign_revocation(request, &key);
    store.admit(&own).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    drop(store);
    let mut store =
        AuthorityStoreRegistered::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    assert!(!active(&mut store, &trust, 3, 4));
    assert!(!active(&mut store, &trust, 6, 7));
    refuse(&mut store, &own);
    refuse(&mut store, &policy(&trust, &key, 3, 4, 9, 2, true));
    refuse(&mut store, &policy(&trust, &other, 6, 7, 10, 2, true));
}

#[test]
fn registered_requests_share_the_operation_namespace_and_authority_revision() {
    let root = root();
    let (trust, key, _) = fixture_registered();
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    store
        .admit(&policy(&trust, &key, 3, 4, 5, 0, true))
        .unwrap();
    refuse(&mut store, &revoke(&trust, &key, 5, 1));
    refuse(&mut store, &revoke(&trust, &key, 8, 0));
    store.admit(&revoke(&trust, &key, 8, 1)).unwrap();
    refuse(&mut store, &policy(&trust, &key, 3, 4, 8, 2, false));
    refuse(&mut store, &policy(&trust, &key, 3, 4, 9, 1, false));
    assert_eq!(
        store
            .admit(&policy(&trust, &key, 3, 4, 9, 2, false))
            .unwrap()
            .revision,
        3
    );
}

#[test]
fn registered_transaction_failures_roll_back_or_require_durable_reconciliation() {
    for point in [
        FaultPoint::AfterInsert,
        FaultPoint::BeforeCommit,
        FaultPoint::AfterCommit,
    ] {
        let root = root();
        let (trust, key, _) = fixture_registered();
        let bootstrap = trust.bootstrap_checkpoint().unwrap();
        let signed = revoke(&trust, &key, 5, 0);
        let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
        let hook: fn(FaultPoint) -> bool = match point {
            FaultPoint::AfterInsert => |current| current == FaultPoint::AfterInsert,
            FaultPoint::BeforeCommit => |current| current == FaultPoint::BeforeCommit,
            FaultPoint::AfterCommit => |current| current == FaultPoint::AfterCommit,
        };
        store.set_fault_hook(Some(hook));
        if point == FaultPoint::AfterCommit {
            assert_eq!(store.admit(&signed), Err(StoreError::CommitUnknown));
            assert_eq!(store.checkpoint(), Err(StoreError::CommitUnknown));
            assert_eq!(
                store.binding_is_active(
                    &trust.base.organization,
                    &trust.policy_admin,
                    &DeviceId::parse(&identifier(6)).unwrap(),
                    &CredentialId::parse(&identifier(7)).unwrap()
                ),
                Err(StoreError::CommitUnknown)
            );
            assert_eq!(store.admit(&signed), Err(StoreError::CommitUnknown));
        } else {
            assert_eq!(store.admit(&signed), Err(StoreError::Unpublished));
            assert_eq!(store.checkpoint().unwrap(), bootstrap);
            assert!(active(&mut store, &trust, 6, 7));
        }
        drop(store);
        let mut store =
            AuthorityStoreRegistered::open_existing(fd(&root), trust.clone(), bootstrap).unwrap();
        assert_eq!(
            active(&mut store, &trust, 6, 7),
            point != FaultPoint::AfterCommit
        );
        assert_eq!(
            store.admit(&signed).unwrap().admission,
            if point == FaultPoint::AfterCommit {
                Admission::Historical
            } else {
                Admission::NewlyAdmitted
            }
        );
        assert!(!active(&mut store, &trust, 6, 7));
    }
}

#[test]
fn registered_profile_refuses_both_legacy_formats_without_conversion() {
    for version in [2, 3, 4] {
        let root = root();
        let (trust, _, _) = fixture_registered();
        let legacy = FixtureTrustV2 {
            base: trust.base.clone(),
            policy_admin: trust.policy_admin.clone(),
        };
        match version {
            2 => drop(AuthorityStore::create_new(fd(&root), trust.base.clone()).unwrap()),
            3 => drop(AuthorityStoreV2::create_new(fd(&root), legacy.clone()).unwrap()),
            4 => drop(AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap()),
            _ => unreachable!(),
        }
        let database = root.path().join("authority.sqlite3");
        let before = fs::read(&database).unwrap();
        assert!(matches!(
            AuthorityStore::create_new(fd(&root), trust.base.clone()),
            Err(StoreError::Refused)
        ));
        assert!(matches!(
            AuthorityStoreV2::create_new(fd(&root), legacy.clone()),
            Err(StoreError::Refused)
        ));
        assert!(matches!(
            AuthorityStoreRegistered::create_new(fd(&root), trust.clone()),
            Err(StoreError::Refused)
        ));
        if version != 2 {
            assert!(matches!(
                AuthorityStore::open_existing(
                    fd(&root),
                    trust.base.clone(),
                    trust.base.bootstrap_checkpoint().unwrap()
                ),
                Err(StoreError::Refused)
            ));
        }
        if version != 3 {
            assert!(matches!(
                AuthorityStoreV2::open_existing(
                    fd(&root),
                    legacy.clone(),
                    legacy.bootstrap_checkpoint().unwrap()
                ),
                Err(StoreError::Refused)
            ));
        }
        if version != 4 {
            assert!(matches!(
                AuthorityStoreRegistered::open_existing(
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
fn registered_schema_has_no_pending_cache_and_queries_preserve_scope() {
    let root = root();
    let (trust, key, _) = fixture_registered();
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
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
    let mut metadata = store.database.prepare("SELECT version FROM meta").unwrap();
    assert!(metadata.row().unwrap());
    assert_eq!(metadata.integer(0).unwrap(), 4);
    assert!(!metadata.row().unwrap());
    drop(metadata);
    store.admit(&revoke(&trust, &key, 5, 0)).unwrap();
    let mut row = store
        .database
        .prepare("SELECT length(request) FROM entries")
        .unwrap();
    assert!(row.row().unwrap());
    assert_eq!(row.integer(0).unwrap(), 226);
    assert!(!row.row().unwrap());
    drop(row);
    let other = OrganizationId::parse(&identifier(9)).unwrap();
    assert!(!store
        .binding_is_active(
            &other,
            &trust.policy_admin,
            &DeviceId::parse(&identifier(3)).unwrap(),
            &CredentialId::parse(&identifier(4)).unwrap()
        )
        .unwrap());
    assert!(!store
        .can_manage_enrollment_policy(&other, &trust.policy_admin)
        .unwrap());
    assert!(!store
        .can_add_own_device(&other, &trust.policy_admin)
        .unwrap());
    assert!(store
        .can_manage_enrollment_policy(&trust.base.organization, &trust.policy_admin)
        .unwrap());
}

#[test]
fn registered_decoder_enforces_disjoint_lengths_and_closed_domains() {
    let (trust, key, _) = fixture_registered();
    let signed = revoke(&trust, &key, 5, 0);
    let bytes = RegisteredProfile::canonical_bytes(&signed);
    let signature = *RegisteredProfile::signature(&signed);
    assert_eq!(
        RegisteredProfile::parse_signed(&bytes, signature).unwrap(),
        signed
    );
    for length in 0..226 {
        assert!(matches!(
            RegisteredProfile::parse_signed(&bytes[..length], signature),
            Err(StoreError::Refused)
        ));
    }
    for length in [214, 215, 225, 227, 8192] {
        assert!(matches!(
            RegisteredProfile::parse_signed(&vec![0; length], signature),
            Err(StoreError::Refused)
        ));
    }
    let mut changed = bytes;
    changed[32] ^= 1;
    assert!(matches!(
        RegisteredProfile::parse_signed(&changed, signature),
        Err(StoreError::Refused)
    ));
    let policy = policy(&trust, &key, 3, 4, 5, 0, true);
    let bytes = RegisteredProfile::canonical_bytes(&policy);
    assert_eq!(
        RegisteredProfile::parse_signed(&bytes, *RegisteredProfile::signature(&policy)).unwrap(),
        policy
    );
    let (legacy_trust, legacy_key, request) = fixture();
    let legacy = super::super::super::tests::sign(request, &legacy_key);
    assert!(matches!(
        RegisteredProfile::parse_signed(&legacy.request.canonical_bytes(), legacy.signature),
        Err(StoreError::Refused)
    ));
    assert!(matches!(
        RegisteredProfile::device(&trust, &policy),
        Err(StoreError::Refused)
    ));
    assert_eq!(legacy_trust.organization, trust.base.organization);
}

#[test]
fn registered_replay_refuses_tampered_rows_metadata_and_extra_schema() {
    for tamper in [
        "UPDATE entries SET signature=zeroblob(64)",
        "UPDATE entries SET operation=CAST('00000000000000000000000009' AS BLOB)",
        "UPDATE entries SET seq=2",
        "UPDATE entries SET request=CAST(replace(CAST(request AS TEXT),'revocation.v1','revocation.v2') AS BLOB)",
        "UPDATE meta SET trust=zeroblob(32)",
        "UPDATE meta SET head=zeroblob(32)",
        "UPDATE meta SET organization=CAST('00000000000000000000000009' AS BLOB)",
        "CREATE TABLE unexpected (value BLOB) STRICT",
        "CREATE INDEX unexpected ON entries(seq)",
        pending::SCHEMA,
    ] {
        let root = root();
        let (trust, key, _) = fixture_registered();
        let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
        store.admit(&revoke(&trust, &key, 5, 0)).unwrap();
        let checkpoint = store.checkpoint().unwrap();
        store.database.execute(tamper).unwrap();
        assert_eq!(store.checkpoint(), Err(StoreError::Refused), "{tamper}");
        drop(store);
        assert!(matches!(AuthorityStoreRegistered::open_existing(fd(&root), trust, checkpoint), Err(StoreError::Refused)), "{tamper}");
    }
}

#[test]
fn registered_invalid_trust_never_creates_a_database() {
    for change in [
        "admin",
        "empty",
        "initial",
        "device",
        "credential",
        "key",
        "organization",
        "bound",
        "public_root",
    ] {
        let root = root();
        let (mut trust, key, other) = fixture_registered();
        let owner = trust.base.initial_principal.clone();
        let organization = trust.base.organization.clone();
        match change {
            "admin" => trust.policy_admin = PrincipalId::parse(&identifier(9)).unwrap(),
            "empty" => trust.base.known.clear(),
            "initial" => {
                trust.base.initial_principal = PrincipalId::parse(&identifier(9)).unwrap();
                trust.policy_admin = trust.base.initial_principal.clone();
            }
            "bound" => trust.base.known = vec![trust.base.known[0].clone(); 129],
            "public_root" => {
                fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap()
            }
            "device" | "credential" | "key" | "organization" => {
                trust.base.known[1] = KnownDevice::new(
                    if change == "organization" {
                        OrganizationId::parse(&identifier(9)).unwrap()
                    } else {
                        organization
                    },
                    owner,
                    DeviceId::parse(&identifier(if change == "device" { 3 } else { 6 })).unwrap(),
                    CredentialId::parse(&identifier(if change == "credential" { 4 } else { 7 }))
                        .unwrap(),
                    if change == "key" {
                        key.verifying_key().to_bytes()
                    } else {
                        other.verifying_key().to_bytes()
                    },
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                AuthorityStoreRegistered::create_new(fd(&root), trust),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
        assert!(!root.path().join("authority.sqlite3").exists());
    }
}

#[test]
fn registered_external_trust_and_retained_checkpoints_refuse_conflicts() {
    let root = root();
    let (trust, key, _) = fixture_registered();
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    store.admit(&revoke(&trust, &key, 5, 0)).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    drop(store);
    let database = root.path().join("authority.sqlite3");
    let before = fs::read(&database).unwrap();
    for change in ["key", "checkpoint", "future"] {
        let mut altered = trust.clone();
        let mut expected = checkpoint;
        match change {
            "key" => {
                altered.base.known[1] = KnownDevice::new(
                    altered.base.organization.clone(),
                    altered.policy_admin.clone(),
                    DeviceId::parse(&identifier(6)).unwrap(),
                    CredentialId::parse(&identifier(7)).unwrap(),
                    SigningKey::from_bytes(&[33; 32]).verifying_key().to_bytes(),
                )
                .unwrap()
            }
            "checkpoint" => expected.digest[0] ^= 1,
            "future" => expected.revision += 1,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                AuthorityStoreRegistered::open_existing(fd(&root), altered, expected),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
        assert_eq!(fs::read(&database).unwrap(), before);
    }
}

#[test]
fn registered_retained_checkpoint_detects_restoring_a_database_before_revocation() {
    let root = root();
    let (trust, key, _) = fixture_registered();
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    let database = root.path().join("authority.sqlite3");
    let old = fs::read(&database).unwrap();
    store.admit(&revoke(&trust, &key, 5, 0)).unwrap();
    let retained = store.checkpoint().unwrap();
    drop(store);
    fs::write(&database, &old).unwrap();
    assert!(matches!(
        AuthorityStoreRegistered::open_existing(fd(&root), trust, retained),
        Err(StoreError::Refused)
    ));
    assert_eq!(fs::read(database).unwrap(), old);
}

#[test]
fn registered_physical_replacement_and_permissions_refuse_live_access() {
    for change in [
        "database",
        "root_permissions",
        "database_permissions",
        "root_capability",
    ] {
        let root = root();
        let replacement = super::super::super::tests::root();
        let (trust, key, _) = fixture_registered();
        let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
        let database = root.path().join("authority.sqlite3");
        match change {
            "database" => {
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
            "root_capability" => {
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
            }
            _ => unreachable!(),
        }
        assert_eq!(store.checkpoint(), Err(StoreError::Refused));
        assert_eq!(
            store.admit(&revoke(&trust, &key, 5, 0)),
            Err(StoreError::Refused)
        );
        assert_eq!(
            store.binding_is_active(
                &trust.base.organization,
                &trust.policy_admin,
                &DeviceId::parse(&identifier(3)).unwrap(),
                &CredentialId::parse(&identifier(4)).unwrap()
            ),
            Err(StoreError::Refused)
        );
    }
}

#[test]
fn registered_header_and_file_size_guards_are_not_weakened() {
    use std::io::{Seek, SeekFrom, Write};

    for change in ["wal", "database_bound", "journal_bound"] {
        let root = root();
        let (trust, _, _) = fixture_registered();
        drop(AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap());
        let database = root.path().join("authority.sqlite3");
        match change {
            "wal" => {
                let mut file = File::options().write(true).open(&database).unwrap();
                file.seek(SeekFrom::Start(18)).unwrap();
                file.write_all(&[2, 2]).unwrap();
            }
            "database_bound" => File::options()
                .write(true)
                .open(&database)
                .unwrap()
                .set_len(MAX_DATABASE as u64 + 1)
                .unwrap(),
            "journal_bound" => {
                let journal = root.path().join("authority.sqlite3-journal");
                let file = File::create(&journal).unwrap();
                file.set_permissions(fs::Permissions::from_mode(0o600))
                    .unwrap();
                file.set_len(MAX_JOURNAL as u64 + 1).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                AuthorityStoreRegistered::open_existing(
                    fd(&root),
                    trust.clone(),
                    trust.bootstrap_checkpoint().unwrap()
                ),
                Err(StoreError::Refused)
            ),
            "{change}"
        );
    }
}

#[test]
fn registered_mixed_history_has_one_operation_bound_and_retains_exact_retries() {
    let root = root();
    let (trust, key, _) = fixture_registered();
    let store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    let mut authority = RegisteredProfile::bootstrap(&trust).unwrap();
    let mut checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut first = None;
    let mut revocation = None;
    store.database.execute("BEGIN IMMEDIATE").unwrap();
    for revision in 0..MAX_OPERATIONS {
        let signed = if revision == 1 {
            revoke(&trust, &key, revision + 100, revision)
        } else {
            policy(&trust, &key, 3, 4, revision + 100, revision, true)
        };
        assert_eq!(
            RegisteredProfile::apply(&trust, &mut authority, &signed).unwrap(),
            Outcome {
                admission: Admission::NewlyAdmitted,
                revision: revision + 1
            }
        );
        if revision == 0 {
            first = Some(signed.clone());
        }
        if revision == 1 {
            revocation = Some(signed.clone());
        }
        let bytes = RegisteredProfile::canonical_bytes(&signed);
        checkpoint = advance(checkpoint, &bytes, RegisteredProfile::signature(&signed));
        let mut insert = store
            .database
            .prepare("INSERT INTO entries VALUES(?1,?2,?3,?4)")
            .unwrap();
        insert.bind_integer(1, checkpoint.revision).unwrap();
        insert
            .bind_blob(2, RegisteredProfile::operation(&signed))
            .unwrap();
        insert.bind_blob(3, &bytes).unwrap();
        insert
            .bind_blob(4, RegisteredProfile::signature(&signed))
            .unwrap();
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
    let mut store =
        AuthorityStoreRegistered::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    assert!(!active(&mut store, &trust, 6, 7));
    refuse(
        &mut store,
        &policy(&trust, &key, 3, 4, 2000, MAX_OPERATIONS, false),
    );
    assert_eq!(
        store.admit(&first.unwrap()).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 1
        }
    );
    assert_eq!(
        store.admit(&revocation.unwrap()).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: 2
        }
    );
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn registered_principal_revocation_invalidates_every_binding_after_replay() {
    let root = root();
    let (trust, key, other) = fixture_registered();
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    let old = policy(&trust, &other, 6, 7, 5, 0, true);
    store.admit(&old).unwrap();
    let mut request = policy_request(policy(&trust, &key, 3, 4, 8, 1, false));
    request.action = V2Action::Classic(Action::RevokePrincipal {
        principal: trust.policy_admin.clone(),
    });
    store.admit(&sign_policy(request, &key)).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    drop(store);
    let mut store =
        AuthorityStoreRegistered::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    assert!(!active(&mut store, &trust, 3, 4));
    assert!(!active(&mut store, &trust, 6, 7));
    assert!(!store
        .can_add_own_device(&trust.base.organization, &trust.policy_admin)
        .unwrap());
    assert!(!store
        .can_manage_enrollment_policy(&trust.base.organization, &trust.policy_admin)
        .unwrap());
    refuse(&mut store, &old);
    refuse(&mut store, &revoke(&trust, &key, 9, 2));
}

#[test]
fn registered_live_admission_rejects_forged_keys_and_mismatched_bindings() {
    let root = root();
    let (trust, key, other) = fixture_registered();
    let mut store = AuthorityStoreRegistered::create_new(fd(&root), trust.clone()).unwrap();
    let request = revocation_request(revoke(&trust, &key, 5, 0));
    refuse(&mut store, &sign_revocation(request.clone(), &other));
    let mut changed = request.clone();
    changed.credential = CredentialId::parse(&identifier(7)).unwrap();
    refuse(&mut store, &sign_revocation(changed, &key));
    let mut changed = request;
    changed.actor = PrincipalId::parse(&identifier(9)).unwrap();
    refuse(&mut store, &sign_revocation(changed, &key));
    refuse(&mut store, &policy(&trust, &other, 3, 4, 5, 0, true));
    assert!(active(&mut store, &trust, 3, 4));
    assert!(active(&mut store, &trust, 6, 7));
}
