use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;

use ed25519_dalek::{Signer, SigningKey};

use super::*;
use crate::team_authority::OrganizationCapability;
use crate::team_authority::{Action, Change, CredentialId, DeviceId, OperationId};

fn id(value: u8) -> String {
    format!("{value:026}")
}

pub(super) fn fixture() -> (FixtureTrust, SigningKey, DeviceRequest) {
    // RFC 8032 section 7.1, TEST 1: published fixture seed, never a credential.
    let seed = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    let bytes = std::array::from_fn(|index| {
        u8::from_str_radix(&seed[index * 2..index * 2 + 2], 16).unwrap()
    });
    let key = SigningKey::from_bytes(&bytes);
    let request = DeviceRequest {
        change: Change {
            organization: OrganizationId::parse(&id(1)).unwrap(),
            actor: PrincipalId::parse(&id(2)).unwrap(),
            operation: OperationId::parse(&id(5)).unwrap(),
            base_revision: 0,
            action: Action::CreateProject {
                project: ProjectId::parse(&id(7)).unwrap(),
            },
        },
        device: DeviceId::parse(&id(3)).unwrap(),
        credential: CredentialId::parse(&id(4)).unwrap(),
    };
    let known = KnownDevice::new(
        request.change.organization.clone(),
        request.change.actor.clone(),
        request.device.clone(),
        request.credential.clone(),
        key.verifying_key().to_bytes(),
    )
    .unwrap();
    (
        FixtureTrust {
            organization: request.change.organization.clone(),
            initial_principal: request.change.actor.clone(),
            known: vec![known],
        },
        key,
        request,
    )
}

pub(super) fn sign(request: DeviceRequest, key: &SigningKey) -> SignedRequest {
    SignedRequest {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    }
}

pub(super) struct TestRoot(std::path::PathBuf);

impl TestRoot {
    pub(super) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn root() -> TestRoot {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = TestRoot(std::env::temp_dir().join(format!(
        "vivac-authority-{}-{stamp}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir(root.path()).unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    root
}

pub(super) fn fd(root: &TestRoot) -> OwnedFd {
    File::open(root.path()).unwrap().into()
}

#[test]
fn v1_profile_control_trace_is_stable() {
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn checkpoint_json(checkpoint: Checkpoint) -> serde_json::Value {
        serde_json::json!({
            "revision": checkpoint.revision,
            "digest": hex(&checkpoint.digest),
        })
    }

    fn outcome_json(outcome: Outcome) -> serde_json::Value {
        serde_json::json!({
            "admission": match outcome.admission {
                Admission::NewlyAdmitted => "newly_admitted",
                Admission::Historical => "historical",
            },
            "revision": outcome.revision,
        })
    }

    let root = root();
    let (trust, key, request) = fixture();
    let project = ProjectId::parse(&id(7)).unwrap();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let bootstrap = store.checkpoint().unwrap();
    assert_eq!(bootstrap, trust.bootstrap_checkpoint().unwrap());
    assert_eq!(bootstrap.revision, 0);

    let signed = sign(request.clone(), &key);
    let created = store.admit(&signed).unwrap();
    assert_eq!(
        created,
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 1,
        }
    );
    let creation = store.checkpoint().unwrap();
    assert_eq!(creation.revision, 1);
    let retried = store.admit(&signed).unwrap();
    assert_eq!(
        retried,
        Outcome {
            admission: Admission::Historical,
            revision: 1,
        }
    );
    assert_eq!(store.checkpoint().unwrap(), creation);

    let mut grant = request.clone();
    grant.change.base_revision = 1;
    grant.change.operation = OperationId::parse(&id(6)).unwrap();
    grant.change.action = Action::SetProjectPermissions {
        principal: trust.initial_principal.clone(),
        project: project.clone(),
        permissions: [ProjectPermission::ReadContent].into_iter().collect(),
    };
    let granted = store.admit(&sign(grant, &key)).unwrap();
    assert_eq!(
        granted,
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 2,
        }
    );
    let permission = store.checkpoint().unwrap();
    assert_eq!(permission.revision, 2);
    let allowed_after_grant = store
        .allows(
            &trust.organization,
            &trust.initial_principal,
            &project,
            ProjectPermission::ReadContent,
        )
        .unwrap();
    assert!(allowed_after_grant);

    let mut revoke = request;
    revoke.change.base_revision = 2;
    revoke.change.operation = OperationId::parse(&id(8)).unwrap();
    revoke.change.action = Action::RevokePrincipal {
        principal: trust.initial_principal.clone(),
    };
    let revoked = store.admit(&sign(revoke, &key)).unwrap();
    assert_eq!(
        revoked,
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 3,
        }
    );
    let revocation = store.checkpoint().unwrap();
    assert_eq!(revocation.revision, 3);
    let allowed_after_revoke = store
        .allows(
            &trust.organization,
            &trust.initial_principal,
            &project,
            ProjectPermission::ReadContent,
        )
        .unwrap();
    assert!(!allowed_after_revoke);
    drop(store);

    let mut reopened = AuthorityStore::open_existing(fd(&root), trust.clone(), revocation).unwrap();
    let reopened_checkpoint = reopened.checkpoint().unwrap();
    assert_eq!(reopened_checkpoint, revocation);
    let allowed_after_reopen = reopened
        .allows(
            &trust.organization,
            &trust.initial_principal,
            &project,
            ProjectPermission::ReadContent,
        )
        .unwrap();
    assert!(!allowed_after_reopen);
    drop(reopened);

    let database = fs::read(root.path().join("authority.sqlite3")).unwrap();
    let database_digest = Sha256::digest(database);
    let trace = serde_json::json!({
        "bootstrap_checkpoint": checkpoint_json(bootstrap),
        "creation_checkpoint": checkpoint_json(creation),
        "creation_outcome": outcome_json(created),
        "retry_outcome": outcome_json(retried),
        "permission_checkpoint": checkpoint_json(permission),
        "permission_outcome": outcome_json(granted),
        "allowed_after_grant": allowed_after_grant,
        "revocation_checkpoint": checkpoint_json(revocation),
        "revocation_outcome": outcome_json(revoked),
        "allowed_after_revoke": allowed_after_revoke,
        "reopened_checkpoint": checkpoint_json(reopened_checkpoint),
        "allowed_after_reopen": allowed_after_reopen,
        "database_sha256": hex(&database_digest),
    });
    crate::output::outln!("V1_CONTROL_TRACE {trace}");
    crate::output::flush();
}

#[test]
fn committed_signed_history_survives_reopen_and_exact_retry() {
    let root = root();
    let (trust, key, request) = fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let signed = sign(request, &key);
    assert_eq!(
        store.admit(&signed).unwrap().admission,
        Admission::NewlyAdmitted
    );
    let checkpoint = store.checkpoint().unwrap();
    assert_eq!(checkpoint.revision, 1);
    drop(store);
    let mut store = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    assert_eq!(
        store.admit(&signed).unwrap().admission,
        Admission::Historical
    );
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn duplicate_external_device_bindings_are_refused() {
    let (mut trust, _, _) = fixture();
    trust.known.push(trust.known[0].clone());
    assert_eq!(trust.bootstrap_checkpoint(), Err(StoreError::Refused));
}

#[test]
fn root_with_public_permissions_is_refused_without_creation() {
    let root = root();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let (trust, _, _) = fixture();
    assert!(matches!(
        AuthorityStore::create_new(fd(&root), trust),
        Err(StoreError::Refused)
    ));
    assert!(!root.path().join("authority.sqlite3").exists());
}

#[test]
fn valid_signature_from_an_unknown_binding_does_not_admit() {
    let root = root();
    let (mut trust, key, request) = fixture();
    trust.known.clear();
    let mut store = AuthorityStore::create_new(fd(&root), trust).unwrap();
    assert_eq!(store.admit(&sign(request, &key)), Err(StoreError::Refused));
    assert_eq!(store.checkpoint().unwrap().revision, 0);
}

#[test]
fn separate_handles_observe_current_history_before_admission() {
    let root = root();
    let (trust, key, request) = fixture();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut first = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let mut second = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    first.admit(&sign(request.clone(), &key)).unwrap();
    let mut competitor = request;
    competitor.change.operation = OperationId::parse(&id(8)).unwrap();
    competitor.change.action = Action::AddPrincipal {
        principal: PrincipalId::parse(&id(9)).unwrap(),
    };
    assert_eq!(
        second.admit(&sign(competitor, &key)),
        Err(StoreError::Refused)
    );
    assert_eq!(second.checkpoint().unwrap(), first.checkpoint().unwrap());
}

#[test]
fn a_newer_checkpoint_refuses_a_restored_old_database() {
    let root = root();
    let (trust, key, request) = fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let backup = fs::read(root.path().join("authority.sqlite3")).unwrap();
    store.admit(&sign(request, &key)).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    drop(store);
    fs::write(root.path().join("authority.sqlite3"), backup).unwrap();
    assert!(matches!(
        AuthorityStore::open_existing(fd(&root), trust, checkpoint),
        Err(StoreError::Refused)
    ));
}

#[test]
fn named_database_replacement_invalidates_a_live_handle() {
    let root = root();
    let (trust, _, _) = fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust).unwrap();
    let database = root.path().join("authority.sqlite3");
    let bytes = fs::read(&database).unwrap();
    fs::rename(&database, root.path().join("old")).unwrap();
    fs::write(&database, bytes).unwrap();
    fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(store.checkpoint(), Err(StoreError::Refused));
}

#[test]
fn symlink_and_hardlink_databases_are_refused() {
    use std::os::unix::fs::symlink;
    let outside = root();
    let root = root();
    let target = outside.path().join("target");
    fs::write(&target, []).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let (trust, _, _) = fixture();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let database = root.path().join("authority.sqlite3");
    symlink(&target, &database).unwrap();
    assert!(matches!(
        AuthorityStore::open_existing(fd(&root), trust.clone(), checkpoint),
        Err(StoreError::Refused)
    ));
    fs::remove_file(&database).unwrap();
    fs::hard_link(&target, &database).unwrap();
    assert!(matches!(
        AuthorityStore::open_existing(fd(&root), trust, checkpoint),
        Err(StoreError::Refused)
    ));
}

#[test]
fn changed_operation_and_bad_signatures_leave_history_unchanged() {
    let root = root();
    let (trust, key, request) = fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust).unwrap();
    let mut invalid = sign(request.clone(), &key);
    invalid.signature[0] ^= 1;
    assert_eq!(store.admit(&invalid), Err(StoreError::Refused));
    assert_eq!(store.checkpoint().unwrap().revision, 0);
    store.admit(&sign(request.clone(), &key)).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    let mut changed = request;
    changed.change.action = Action::AddPrincipal {
        principal: PrincipalId::parse(&id(9)).unwrap(),
    };
    assert_eq!(store.admit(&sign(changed, &key)), Err(StoreError::Refused));
    assert_eq!(store.checkpoint().unwrap(), checkpoint);
}

#[test]
fn a_conflicting_checkpoint_refuses_even_at_the_same_revision() {
    let root = root();
    let (trust, _, _) = fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let mut checkpoint = store.checkpoint().unwrap();
    checkpoint.digest[0] ^= 1;
    drop(store);
    assert!(matches!(
        AuthorityStore::open_existing(fd(&root), trust, checkpoint),
        Err(StoreError::Refused)
    ));
}

#[test]
fn truncated_database_is_not_repaired_or_recreated() {
    let root = root();
    let (trust, _, _) = fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    drop(store);
    let database = root.path().join("authority.sqlite3");
    fs::OpenOptions::new()
        .write(true)
        .open(&database)
        .unwrap()
        .set_len(64)
        .unwrap();
    assert!(AuthorityStore::open_existing(fd(&root), trust, checkpoint).is_err());
    assert_eq!(fs::metadata(database).unwrap().len(), 64);
}

#[test]
fn revocation_survives_restart_and_refuses_old_receipts() {
    let root = root();
    let (trust, key, request) = fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let original = sign(request.clone(), &key);
    store.admit(&original).unwrap();
    let mut grant = request.clone();
    grant.change.base_revision = 1;
    grant.change.operation = OperationId::parse(&id(8)).unwrap();
    grant.change.action = Action::SetProjectPermissions {
        principal: trust.initial_principal.clone(),
        project: ProjectId::parse(&id(7)).unwrap(),
        permissions: [ProjectPermission::ReadContent].into_iter().collect(),
    };
    store.admit(&sign(grant, &key)).unwrap();
    assert!(store
        .allows(
            &trust.organization,
            &trust.initial_principal,
            &ProjectId::parse(&id(7)).unwrap(),
            ProjectPermission::ReadContent
        )
        .unwrap());
    let mut revoke = request;
    revoke.change.base_revision = 2;
    revoke.change.operation = OperationId::parse(&id(9)).unwrap();
    revoke.change.action = Action::RevokePrincipal {
        principal: trust.initial_principal.clone(),
    };
    store.admit(&sign(revoke, &key)).unwrap();
    let checkpoint = store.checkpoint().unwrap();
    drop(store);
    let mut reopened = AuthorityStore::open_existing(fd(&root), trust.clone(), checkpoint).unwrap();
    assert_eq!(reopened.admit(&original), Err(StoreError::Refused));
    assert!(!reopened
        .allows(
            &trust.organization,
            &trust.initial_principal,
            &ProjectId::parse(&id(7)).unwrap(),
            ProjectPermission::ReadContent
        )
        .unwrap());
}

#[test]
fn precommit_failures_roll_back_but_postcommit_failure_poisoned() {
    for point in [
        FaultPoint::AfterInsert,
        FaultPoint::BeforeCommit,
        FaultPoint::AfterCommit,
    ] {
        let root = root();
        let (trust, key, request) = fixture();
        let checkpoint = trust.bootstrap_checkpoint().unwrap();
        let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
        let hook: fn(FaultPoint) -> bool = match point {
            FaultPoint::AfterInsert => |current| current == FaultPoint::AfterInsert,
            FaultPoint::BeforeCommit => |current| current == FaultPoint::BeforeCommit,
            FaultPoint::AfterCommit => |current| current == FaultPoint::AfterCommit,
        };
        store.set_fault_hook(Some(hook));
        let request = sign(request, &key);
        let result = store.admit(&request);
        if point == FaultPoint::AfterCommit {
            assert_eq!(result, Err(StoreError::CommitUnknown));
            assert_eq!(store.checkpoint(), Err(StoreError::CommitUnknown));
        } else {
            assert_eq!(result, Err(StoreError::Unpublished));
            assert_eq!(store.checkpoint().unwrap(), checkpoint);
        }
        drop(store);
        let mut reopened = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
        assert_eq!(
            reopened.checkpoint().unwrap().revision,
            u64::from(point == FaultPoint::AfterCommit)
        );
        assert_eq!(
            reopened.admit(&request).unwrap().admission,
            if point == FaultPoint::AfterCommit {
                Admission::Historical
            } else {
                Admission::NewlyAdmitted
            }
        );
    }
}

#[test]
fn changed_signed_rows_and_unknown_schema_are_refused() {
    for tamper in [
        "UPDATE entries SET signature=zeroblob(64)",
        "UPDATE meta SET head=zeroblob(32)",
        "CREATE TABLE unexpected (value BLOB) STRICT",
    ] {
        let root = root();
        let (trust, key, request) = fixture();
        let mut store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
        store.admit(&sign(request, &key)).unwrap();
        let checkpoint = store.checkpoint().unwrap();
        store.database.execute(tamper).unwrap();
        assert_eq!(store.checkpoint(), Err(StoreError::Refused));
        drop(store);
        assert!(AuthorityStore::open_existing(fd(&root), trust, checkpoint).is_err());
    }
}

#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("VIVAC_AUTHORITY_CRASH_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let (trust, key, request) = fixture();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut store =
        AuthorityStore::open_existing(File::open(root).unwrap().into(), trust, checkpoint).unwrap();
    store.set_fault_hook(Some(|point| {
        let target = match std::env::var("VIVAC_AUTHORITY_CRASH_POINT")
            .unwrap()
            .as_str()
        {
            "before" => FaultPoint::BeforeCommit,
            "after" => FaultPoint::AfterCommit,
            _ => panic!("Invalid crash fixture point"),
        };
        if point == target {
            unsafe { libc::_exit(77) };
        }
        false
    }));
    let _ = store.admit(&sign(request, &key));
    panic!("Crash fixture did not reach its fault point");
}

#[test]
fn process_crashes_before_and_after_commit_reconcile_on_reopen() {
    for point in ["before", "after"] {
        let root = root();
        let (trust, key, request) = fixture();
        let checkpoint = trust.bootstrap_checkpoint().unwrap();
        drop(AuthorityStore::create_new(fd(&root), trust.clone()).unwrap());
        let test_module = module_path!().split_once("::").unwrap().1;
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(format!("{test_module}::crash_child"))
            .env("VIVAC_AUTHORITY_CRASH_ROOT", root.path())
            .env("VIVAC_AUTHORITY_CRASH_POINT", point)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77));
        let mut reopened = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
        assert_eq!(
            reopened.checkpoint().unwrap().revision,
            u64::from(point == "after")
        );
        assert_eq!(
            reopened.admit(&sign(request, &key)).unwrap().admission,
            if point == "after" {
                Admission::Historical
            } else {
                Admission::NewlyAdmitted
            }
        );
    }
}

#[test]
fn descriptor_flags_do_not_bypass_root_policy() {
    let root = root();
    let (trust, _, _) = fixture();
    let ordinary = fd(&root);
    assert_eq!(
        unsafe { libc::fcntl(ordinary.as_raw_fd(), libc::F_SETFD, 0) },
        0
    );
    assert!(matches!(
        AuthorityStore::create_new(ordinary, trust.clone()),
        Err(StoreError::Refused)
    ));
    let path = CString::new(root.path().as_os_str().as_encoded_bytes()).unwrap();
    let raw = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    assert!(raw >= 3);
    let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
    assert!(matches!(
        AuthorityStore::create_new(descriptor, trust),
        Err(StoreError::Refused)
    ));
    assert!(!root.path().join("authority.sqlite3").exists());
}

#[test]
fn unsupported_namespace_objects_and_oversize_journals_refuse() {
    for name in [
        "authority.sqlite3-wal",
        "authority.sqlite3-shm",
        "extra",
        "authority.sqlite3-journal",
    ] {
        let root = root();
        let (trust, _, _) = fixture();
        let checkpoint = trust.bootstrap_checkpoint().unwrap();
        drop(AuthorityStore::create_new(fd(&root), trust.clone()).unwrap());
        let path = root.path().join(name);
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        if name.ends_with("-journal") {
            file.set_len(MAX_JOURNAL as u64 + 1).unwrap();
        }
        drop(file);
        assert!(matches!(
            AuthorityStore::open_existing(fd(&root), trust, checkpoint),
            Err(StoreError::Refused)
        ));
        assert!(path.exists());
    }
}

fn install_private_acl(path: &std::path::Path, owner_permissions: u16) {
    install_acl(path, owner_permissions, c"system.posix_acl_access");
}

fn install_acl(path: &std::path::Path, owner_permissions: u16, name: &CStr) {
    let mut bytes = 2_u32.to_le_bytes().to_vec();
    let other_user = if unsafe { libc::geteuid() } == 65534 {
        65533
    } else {
        65534
    };
    for (tag, permissions, identifier) in [
        (1_u16, owner_permissions, u32::MAX),
        (2, 0, other_user),
        (4, 0, u32::MAX),
        (16, 0, u32::MAX),
        (32, 0, u32::MAX),
    ] {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&permissions.to_le_bytes());
        bytes.extend_from_slice(&identifier.to_le_bytes());
    }
    let path = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            name.as_ptr(),
            bytes.as_ptr().cast(),
            bytes.len(),
            0,
        )
    };
    assert_eq!(
        result,
        0,
        "The ext fixture must support POSIX ACLs: {}",
        std::io::Error::last_os_error()
    );
}

#[test]
fn restrictive_extended_acls_still_refuse() {
    for root_acl in [true, false] {
        let root = root();
        let (trust, _, _) = fixture();
        if root_acl {
            install_private_acl(root.path(), 7);
            assert_eq!(
                fs::metadata(root.path()).unwrap().permissions().mode() & 0o7777,
                0o700
            );
            assert!(matches!(
                AuthorityStore::create_new(fd(&root), trust),
                Err(StoreError::Refused)
            ));
        } else {
            let checkpoint = trust.bootstrap_checkpoint().unwrap();
            drop(AuthorityStore::create_new(fd(&root), trust.clone()).unwrap());
            let database = root.path().join("authority.sqlite3");
            install_private_acl(&database, 6);
            assert_eq!(
                fs::metadata(&database).unwrap().permissions().mode() & 0o7777,
                0o600
            );
            assert!(matches!(
                AuthorityStore::open_existing(fd(&root), trust, checkpoint),
                Err(StoreError::Refused)
            ));
        }
    }
}

#[test]
fn external_bindings_are_bounded_and_order_independent() {
    let (mut trust, key, _) = fixture();
    trust.known.clear();
    for index in 0..128 {
        trust.known.push(
            KnownDevice::new(
                trust.organization.clone(),
                trust.initial_principal.clone(),
                DeviceId::parse(&format!("{index:026}")).unwrap(),
                CredentialId::parse(&id(4)).unwrap(),
                key.verifying_key().to_bytes(),
            )
            .unwrap(),
        );
    }
    let expected = trust.bootstrap_checkpoint().unwrap();
    trust.known.reverse();
    assert_eq!(trust.bootstrap_checkpoint().unwrap(), expected);
    trust.known.push(
        KnownDevice::new(
            trust.organization.clone(),
            trust.initial_principal.clone(),
            DeviceId::parse(&format!("{:026}", 128)).unwrap(),
            CredentialId::parse(&id(4)).unwrap(),
            key.verifying_key().to_bytes(),
        )
        .unwrap(),
    );
    assert_eq!(trust.bootstrap_checkpoint(), Err(StoreError::Refused));
}

#[test]
fn full_signed_history_keeps_exact_retry_and_refuses_operation_1025() {
    let root = root();
    let (trust, key, mut request) = fixture();
    let store = AuthorityStore::create_new(fd(&root), trust.clone()).unwrap();
    let mut checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut authority =
        SignedAuthority::bootstrap(trust.organization.clone(), trust.initial_principal.clone());
    let mut first = None;
    store.database.execute("BEGIN IMMEDIATE").unwrap();
    for revision in 0..MAX_OPERATIONS {
        request.change.base_revision = revision;
        request.change.operation = OperationId::parse(&format!("{:026}", revision + 100)).unwrap();
        request.change.action = Action::SetOrganizationCapabilities {
            principal: trust.initial_principal.clone(),
            capabilities: [
                OrganizationCapability::ManageMembers,
                OrganizationCapability::ManageProjects,
                OrganizationCapability::GrantProjectAccess,
                OrganizationCapability::ManageOrganizationCapabilities,
            ]
            .into_iter()
            .collect(),
        };
        let signed = sign(request.clone(), &key);
        assert_eq!(
            authority
                .apply_signed(&signed, &trust.known[0])
                .unwrap()
                .admission,
            Admission::NewlyAdmitted
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
            .bind_blob(2, request.change.operation.as_str().as_bytes())
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
    let mut reopened = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    request.change.base_revision = MAX_OPERATIONS;
    request.change.operation =
        OperationId::parse(&format!("{:026}", MAX_OPERATIONS + 100)).unwrap();
    assert_eq!(
        reopened.admit(&sign(request, &key)),
        Err(StoreError::Refused)
    );
    assert_eq!(
        reopened.admit(&first.unwrap()).unwrap().admission,
        Admission::Historical
    );
    assert_eq!(reopened.checkpoint().unwrap(), checkpoint);
}

fn wait_for(path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "Process fixture rendezvous timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

struct OwnedChild {
    process: std::process::Child,
    waited: bool,
}

impl OwnedChild {
    fn wait(&mut self) -> std::process::ExitStatus {
        let status = self.process.wait().unwrap();
        self.waited = true;
        status
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.waited {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }
}

#[test]
fn competing_process_child() {
    let Some(root) = std::env::var_os("VIVAC_AUTHORITY_RACE_ROOT") else {
        return;
    };
    let control =
        std::path::PathBuf::from(std::env::var_os("VIVAC_AUTHORITY_RACE_CONTROL").unwrap());
    let role = std::env::var("VIVAC_AUTHORITY_RACE_ROLE").unwrap();
    let (trust, key, mut request) = fixture();
    if role == "second" {
        request.change.operation = OperationId::parse(&id(8)).unwrap();
        request.change.action = Action::AddPrincipal {
            principal: PrincipalId::parse(&id(9)).unwrap(),
        };
    }
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    let mut store =
        AuthorityStore::open_existing(File::open(root).unwrap().into(), trust, checkpoint).unwrap();
    fs::write(control.join(format!("ready-{role}")), []).unwrap();
    wait_for(&control.join("go"));
    let outcome = match store.admit(&sign(request, &key)) {
        Ok(Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 1,
        }) => "new",
        Err(StoreError::Refused) => "refused",
        Err(StoreError::Unpublished) => "unpublished",
        other => panic!("Unexpected competing admission: {other:?}"),
    };
    fs::write(control.join(format!("result-{role}")), outcome).unwrap();
}

#[test]
fn two_processes_admit_exactly_one_predecessor_successor() {
    let control = root();
    let root = root();
    let (trust, key, request) = fixture();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    drop(AuthorityStore::create_new(fd(&root), trust.clone()).unwrap());
    let test_module = module_path!().split_once("::").unwrap().1;
    let spawn = |role: &str| OwnedChild {
        process: std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(format!("{test_module}::competing_process_child"))
            .env("VIVAC_AUTHORITY_RACE_ROOT", root.path())
            .env("VIVAC_AUTHORITY_RACE_CONTROL", control.path())
            .env("VIVAC_AUTHORITY_RACE_ROLE", role)
            .spawn()
            .unwrap(),
        waited: false,
    };
    let mut first = spawn("first");
    let mut second = spawn("second");
    wait_for(&control.path().join("ready-first"));
    wait_for(&control.path().join("ready-second"));
    fs::write(control.path().join("go"), []).unwrap();
    assert!(first.wait().success());
    assert!(second.wait().success());
    let results = [
        fs::read_to_string(control.path().join("result-first")).unwrap(),
        fs::read_to_string(control.path().join("result-second")).unwrap(),
    ];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.as_str() == "new")
            .count(),
        1
    );
    let mut reopened = AuthorityStore::open_existing(fd(&root), trust, checkpoint).unwrap();
    assert_eq!(reopened.checkpoint().unwrap().revision, 1);
    let mut competitor = request.clone();
    competitor.change.operation = OperationId::parse(&id(8)).unwrap();
    competitor.change.action = Action::AddPrincipal {
        principal: PrincipalId::parse(&id(9)).unwrap(),
    };
    let requests = [sign(request, &key), sign(competitor, &key)];
    for (result, request) in results.iter().zip(requests) {
        if result == "new" {
            assert_eq!(
                reopened.admit(&request).unwrap().admission,
                Admission::Historical
            );
        } else {
            assert_eq!(reopened.admit(&request), Err(StoreError::Refused));
        }
    }
}

#[test]
fn default_root_acl_refuses_without_creating_database() {
    let root = root();
    let (trust, _, _) = fixture();
    install_acl(root.path(), 7, c"system.posix_acl_default");
    assert_eq!(
        fs::metadata(root.path()).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    assert!(matches!(
        AuthorityStore::create_new(fd(&root), trust),
        Err(StoreError::Refused)
    ));
    assert!(!root.path().join("authority.sqlite3").exists());
}

#[test]
fn held_write_transaction_refuses_child_admission_without_publication() {
    let control = root();
    let root = root();
    let (trust, key, request) = fixture();
    let mut store = AuthorityStore::create_new(fd(&root), trust).unwrap();
    store.database.execute("BEGIN IMMEDIATE").unwrap();
    let test_module = module_path!().split_once("::").unwrap().1;
    let mut child = OwnedChild {
        process: std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(format!("{test_module}::competing_process_child"))
            .env("VIVAC_AUTHORITY_RACE_ROOT", root.path())
            .env("VIVAC_AUTHORITY_RACE_CONTROL", control.path())
            .env("VIVAC_AUTHORITY_RACE_ROLE", "first")
            .spawn()
            .unwrap(),
        waited: false,
    };
    wait_for(&control.path().join("ready-first"));
    fs::write(control.path().join("go"), []).unwrap();
    assert!(child.wait().success());
    assert_eq!(
        fs::read_to_string(control.path().join("result-first")).unwrap(),
        "unpublished"
    );
    store.rollback().unwrap();
    assert_eq!(store.checkpoint().unwrap().revision, 0);
    assert_eq!(
        store.admit(&sign(request, &key)).unwrap().admission,
        Admission::NewlyAdmitted
    );
}

#[test]
fn wal_header_refuses_without_creating_auxiliary_files() {
    use std::io::{Read, Seek, SeekFrom, Write};
    let root = root();
    let (trust, _, _) = fixture();
    let checkpoint = trust.bootstrap_checkpoint().unwrap();
    drop(AuthorityStore::create_new(fd(&root), trust.clone()).unwrap());
    let path = root.path().join("authority.sqlite3");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(18)).unwrap();
    file.write_all(&[2, 2]).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let raw = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    assert!(raw >= 3);
    let notifications = unsafe { OwnedFd::from_raw_fd(raw) };
    let directory = CString::new(root.path().as_os_str().as_encoded_bytes()).unwrap();
    assert!(
        unsafe {
            libc::inotify_add_watch(
                notifications.as_raw_fd(),
                directory.as_ptr(),
                libc::IN_CREATE,
            )
        } >= 0
    );
    assert!(matches!(
        AuthorityStore::open_existing(fd(&root), trust, checkpoint),
        Err(StoreError::Refused)
    ));
    let mut events = [0_u8; 4096];
    let count = unsafe {
        libc::read(
            notifications.as_raw_fd(),
            events.as_mut_ptr().cast(),
            events.len(),
        )
    };
    if count >= 0 {
        let mut offset = 0;
        while offset < count as usize {
            let event_size = std::mem::size_of::<libc::inotify_event>();
            assert!(offset + event_size <= count as usize);
            let event = unsafe {
                ptr::read_unaligned(events.as_ptr().add(offset).cast::<libc::inotify_event>())
            };
            let start = offset + event_size;
            let end = start + event.len as usize;
            assert!(end <= count as usize);
            let name = &events[start..end];
            assert!(
                !name.starts_with(b"authority.sqlite3-wal\0")
                    && !name.starts_with(b"authority.sqlite3-shm\0"),
                "Refusing WAL must not create transient WAL or SHM files"
            );
            offset = end;
        }
    } else {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EAGAIN)
        );
    }
    assert!(!root.path().join("authority.sqlite3-wal").exists());
    assert!(!root.path().join("authority.sqlite3-shm").exists());
    let mut file = File::open(path).unwrap();
    file.seek(SeekFrom::Start(18)).unwrap();
    let mut header = [0; 2];
    file.read_exact(&mut header).unwrap();
    assert_eq!(header, [2, 2]);
}
