use ed25519_dalek::{Signer, SigningKey};

use super::*;

fn organization() -> OrganizationId {
    OrganizationId::parse("00000000000000000000000001").unwrap()
}

fn principal(value: u64) -> PrincipalId {
    PrincipalId::parse(&format!("{value:026}")).unwrap()
}

fn device(value: u64) -> DeviceId {
    DeviceId::parse(&format!("{value:026}")).unwrap()
}

fn credential(value: u64) -> CredentialId {
    CredentialId::parse(&format!("{value:026}")).unwrap()
}

fn operation(value: u64) -> OperationId {
    OperationId::parse(&format!("{value:026}")).unwrap()
}

fn fixture_key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 1: this is a published test seed.
    SigningKey::from_bytes(&[
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ])
}

fn other_key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 2: this is a published test seed.
    SigningKey::from_bytes(&[
        0x4c, 0xcd, 0x08, 0x9b, 0x28, 0xff, 0x96, 0xda, 0x9d, 0xb6, 0xc3, 0x46, 0xec, 0x11, 0x4e,
        0x0f, 0x5b, 0x8a, 0x31, 0x9f, 0x35, 0xab, 0xa6, 0x24, 0xda, 0x8c, 0xf6, 0xed, 0x4f, 0xb8,
        0xa6, 0xfb,
    ])
}

fn known(owner: u64, device_id: u64, credential_id: u64, key: &SigningKey) -> KnownDevice {
    KnownDevice::new(
        organization(),
        principal(owner),
        device(device_id),
        credential(credential_id),
        key.verifying_key().to_bytes(),
    )
    .unwrap()
}

fn authority() -> RegisteredAuthority {
    RegisteredAuthority::bootstrap_fixture(
        organization(),
        principal(2),
        principal(2),
        vec![known(2, 3, 4, &fixture_key()), known(2, 6, 7, &other_key())],
    )
    .unwrap()
}

fn revocation() -> RevocationRequest {
    RevocationRequest {
        organization: organization(),
        actor: principal(2),
        device: device(3),
        credential: credential(4),
        operation: operation(5),
        base_revision: 0,
        target_device: device(6),
        target_credential: credential(7),
    }
}

fn sign_revocation(request: RevocationRequest, key: &SigningKey) -> SignedRevocation {
    SignedRevocation {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    }
}

fn third_key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 3: this is a published test seed.
    SigningKey::from_bytes(&[
        0xc5, 0xaa, 0x8d, 0xf4, 0x3f, 0x9f, 0x83, 0x7b, 0xed, 0xb7, 0x44, 0x2f, 0x31, 0xdc, 0xb7,
        0xb1, 0x66, 0xd3, 0x85, 0x35, 0x07, 0x6f, 0x09, 0x4b, 0x85, 0xce, 0x3a, 0x2e, 0x0b, 0x44,
        0x58, 0xf7,
    ])
}

fn policy_request(revision: u64, operation_id: u64) -> DeviceRequestV2 {
    DeviceRequestV2 {
        organization: organization(),
        actor: principal(2),
        device: device(3),
        credential: credential(4),
        operation: operation(operation_id),
        base_revision: revision,
        action: V2Action::SetOwnAdditionalDevice {
            principal: principal(2),
            enabled: true,
        },
    }
}

fn sign_policy(request: DeviceRequestV2, key: &SigningKey) -> SignedRequestV2 {
    SignedRequestV2 {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    }
}

fn apply_policy(current: &mut RegisteredAuthority, action: V2Action) -> SignedRequestV2 {
    let mut request = policy_request(current.revision(), current.revision() + 100);
    request.action = action;
    let signed = sign_policy(request, &fixture_key());
    let revision = current.revision() + 1;
    assert_eq!(
        current.apply_signed(&signed),
        Ok(Outcome {
            admission: Admission::NewlyAdmitted,
            revision,
        })
    );
    signed
}

fn refuse_revocation(current: &mut RegisteredAuthority, signed: &SignedRevocation) {
    let before = format!("{current:?}");
    assert_eq!(current.apply_revocation(signed), Err(Refused));
    assert_eq!(format!("{current:?}"), before);
}

fn refuse_policy(current: &mut RegisteredAuthority, signed: &SignedRequestV2) {
    let before = format!("{current:?}");
    assert_eq!(current.apply_signed(signed), Err(Refused));
    assert_eq!(format!("{current:?}"), before);
}

fn bootstrap(bindings: Vec<KnownDevice>) -> Result<RegisteredAuthority> {
    RegisteredAuthority::bootstrap_fixture(organization(), principal(2), principal(2), bindings)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn own_revocation_keeps_the_other_device_active() {
    let mut current = authority();
    let signed = sign_revocation(revocation(), &fixture_key());
    assert_eq!(
        current.apply_revocation(&signed),
        Ok(Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 1,
        })
    );
    assert!(current
        .known_device(&organization(), &principal(2), &device(6), &credential(7))
        .is_err());
    assert!(current
        .known_device(&organization(), &principal(2), &device(3), &credential(4))
        .is_ok());
    assert_eq!(current.revision(), 1);
}

#[test]
fn registered_queries_keep_project_policy_and_candidate_authority_separate() {
    let mut current = authority();
    let project = ProjectId::parse("00000000000000000000000042").unwrap();
    let candidate_key = third_key().verifying_key().to_bytes();
    assert!(current.can_manage_enrollment_policy(&organization(), &principal(2)));
    assert!(!current.can_add_own_device(&organization(), &principal(2)));
    assert!(!current.allows(
        &organization(),
        &principal(2),
        &project,
        ProjectPermission::ReadContent
    ));
    assert!(current
        .validate_additional_candidate(
            &organization(),
            &principal(2),
            &device(9),
            &credential(10),
            candidate_key
        )
        .is_err());
    let created = apply_policy(
        &mut current,
        V2Action::Classic(Action::CreateProject {
            project: project.clone(),
        }),
    );
    let permissions = apply_policy(
        &mut current,
        V2Action::Classic(Action::SetProjectPermissions {
            principal: principal(2),
            project: project.clone(),
            permissions: [ProjectPermission::ReadContent].into_iter().collect(),
        }),
    );
    assert!(current.allows(
        &organization(),
        &principal(2),
        &project,
        ProjectPermission::ReadContent
    ));
    assert!(!current.can_add_own_device(&organization(), &principal(2)));
    assert!(current
        .validate_additional_candidate(
            &organization(),
            &principal(2),
            &device(9),
            &credential(10),
            candidate_key
        )
        .is_err());
    let granted = apply_policy(
        &mut current,
        V2Action::SetOwnAdditionalDevice {
            principal: principal(2),
            enabled: true,
        },
    );
    assert!(current.can_add_own_device(&organization(), &principal(2)));
    let before = format!("{current:?}");
    assert!(current
        .validate_additional_candidate(
            &organization(),
            &principal(2),
            &device(9),
            &credential(10),
            candidate_key
        )
        .is_ok());
    for (device_id, credential_id, key) in [
        (3, 10, candidate_key),
        (9, 4, candidate_key),
        (9, 10, fixture_key().verifying_key().to_bytes()),
    ] {
        assert!(current
            .validate_additional_candidate(
                &organization(),
                &principal(2),
                &device(device_id),
                &credential(credential_id),
                key
            )
            .is_err());
    }
    assert_eq!(format!("{current:?}"), before);
    for signed in [&created, &permissions, &granted] {
        assert!(current.operation_is_known(&signed.request.operation));
    }
    assert!(!current.operation_is_known(&operation(999)));
    let mut request = revocation();
    request.base_revision = current.revision();
    let signed = sign_revocation(request, &fixture_key());
    current.apply_revocation(&signed).unwrap();
    assert!(current.operation_is_known(&signed.request.operation));
    assert!(current
        .validate_additional_candidate(
            &organization(),
            &principal(2),
            &device(6),
            &credential(11),
            candidate_key
        )
        .is_err());
    assert!(current.allows(
        &organization(),
        &principal(2),
        &project,
        ProjectPermission::ReadContent
    ));
}

#[test]
fn bootstrap_requires_initial_binding_and_unique_device_credential_and_key() {
    assert!(bootstrap(vec![]).is_err());
    assert!(bootstrap(vec![known(8, 3, 4, &fixture_key())]).is_err());
    let first = known(2, 3, 4, &fixture_key());
    for second in [
        first.clone(),
        known(8, 3, 7, &other_key()),
        known(8, 6, 4, &other_key()),
        known(8, 6, 7, &fixture_key()),
    ] {
        assert!(bootstrap(vec![first.clone(), second]).is_err());
    }
    let mut foreign = known(2, 6, 7, &other_key());
    foreign.organization = OrganizationId::parse("00000000000000000000000009").unwrap();
    assert!(bootstrap(vec![first, foreign]).is_err());
    assert!(RegisteredAuthority::bootstrap_fixture(
        organization(),
        principal(2),
        principal(8),
        vec![known(2, 3, 4, &fixture_key())],
    )
    .is_err());
    assert!(KnownDevice::new(
        organization(),
        principal(2),
        device(3),
        credential(4),
        [0; 32],
    )
    .is_err());
}

#[test]
fn bootstrap_binding_bound_includes_inactive_fixtures() {
    // These deterministic seeds are public test fixtures, not deployed keys.
    let bindings: Vec<_> = (0..128)
        .map(|index| {
            known(
                if index == 0 { 2 } else { 8 },
                index + 100,
                index + 300,
                &SigningKey::from_bytes(&[index as u8; 32]),
            )
        })
        .collect();
    assert!(bootstrap(bindings.clone()).is_ok());
    let mut excessive = bindings;
    excessive.push(known(8, 999, 999, &fixture_key()));
    assert!(bootstrap(excessive).is_err());
}

#[test]
fn predeclared_binding_is_unusable_until_its_principal_is_active() {
    let mut current = bootstrap(vec![
        known(2, 3, 4, &fixture_key()),
        known(8, 6, 7, &other_key()),
    ])
    .unwrap();
    let mut request = policy_request(0, 50);
    request.actor = principal(8);
    request.device = device(6);
    request.credential = credential(7);
    request.action = V2Action::Classic(Action::CreateProject {
        project: ProjectId::parse("00000000000000000000000009").unwrap(),
    });
    refuse_policy(&mut current, &sign_policy(request, &other_key()));
    assert!(current
        .known_device(&organization(), &principal(8), &device(6), &credential(7))
        .is_err());
    apply_policy(
        &mut current,
        V2Action::Classic(Action::AddPrincipal {
            principal: principal(8),
        }),
    );
    assert!(current
        .known_device(&organization(), &principal(8), &device(6), &credential(7))
        .is_ok());
}

#[test]
fn current_binding_lookup_checks_every_identity_and_revoked_state() {
    let mut current = authority();
    assert!(current
        .known_device(
            &OrganizationId::parse("00000000000000000000000009").unwrap(),
            &principal(2),
            &device(3),
            &credential(4),
        )
        .is_err());
    for (owner, device_id, credential_id) in [(8, 3, 4), (2, 6, 4), (2, 3, 7)] {
        assert!(current
            .known_device(
                &organization(),
                &principal(owner),
                &device(device_id),
                &credential(credential_id),
            )
            .is_err());
    }
    current
        .apply_revocation(&sign_revocation(revocation(), &fixture_key()))
        .unwrap();
    assert!(current.bindings[1].revoked);
    assert_eq!(current.bindings.len(), 2);
    assert_eq!(
        current.bindings[1].known.key.to_bytes(),
        other_key().verifying_key().to_bytes()
    );
}

#[test]
fn revocation_codec_matches_the_independent_published_fixture_vector() {
    use sha2::{Digest, Sha256};

    let request = revocation();
    let bytes = request.canonical_bytes();
    assert_eq!(DOMAIN.len(), 36);
    assert_eq!(bytes.len(), 226);
    assert_eq!(REVOCATION_BYTES, 226);
    assert_eq!(&bytes[..36], b"vivac.team.credential-revocation.v1\0");
    assert_eq!(&bytes[36..62], b"00000000000000000000000001");
    assert_eq!(&bytes[166..174], &[0; 8]);
    assert_eq!(&bytes[174..200], b"00000000000000000000000006");
    assert_eq!(&bytes[200..], b"00000000000000000000000007");
    assert_eq!(
        hex(&Sha256::digest(&bytes)),
        "f0687535353d7825c9468a115f3f8ccab982d17d523a1f656933301517db54fc"
    );
    assert_eq!(
        hex(&sign_revocation(request.clone(), &fixture_key()).signature),
        "479267f76861fdf6b6777194ae3b11c79e0ffbc36f6c96afecb7a49b542776eda23a1016dadc4c3775fe501ea1125db269339c20fdeb69ff9a2720b199ca000f"
    );
    assert_eq!(RevocationRequest::parse_canonical(&bytes), Ok(request));
    let mut request = revocation();
    request.base_revision = 0x0102_0304_0506_0708;
    let bytes = request.canonical_bytes();
    assert_eq!(&bytes[166..174], &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(RevocationRequest::parse_canonical(&bytes), Ok(request));
}

#[test]
fn revocation_codec_refuses_other_domains_bad_identifiers_and_extra_bytes() {
    let bytes = revocation().canonical_bytes();
    for length in 0..bytes.len() {
        assert!(RevocationRequest::parse_canonical(&bytes[..length]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(RevocationRequest::parse_canonical(&trailing).is_err());
    let mut domain = bytes.clone();
    domain[32] = b'2';
    assert!(RevocationRequest::parse_canonical(&domain).is_err());
    for offset in [36, 62, 88, 114, 140, 174, 200] {
        for invalid in [b'8', b'A', b'i', 0xff] {
            let mut malformed = bytes.clone();
            malformed[offset] = invalid;
            assert!(RevocationRequest::parse_canonical(&malformed).is_err());
        }
    }
    let policy = policy_request(0, 5).canonical_bytes();
    assert!(RevocationRequest::parse_canonical(&policy).is_err());
    assert!(DeviceRequestV2::parse_canonical(&bytes).is_err());
    assert!(DeviceRequest::parse_canonical(&bytes).is_err());
}

#[test]
fn every_signed_identity_and_target_is_checked_without_mutation() {
    let original = revocation();
    let mut requests = Vec::new();
    let mut changed = original.clone();
    changed.organization = OrganizationId::parse("00000000000000000000000009").unwrap();
    requests.push(changed);
    let mut changed = original.clone();
    changed.actor = principal(8);
    requests.push(changed);
    let mut changed = original.clone();
    changed.device = device(6);
    requests.push(changed);
    let mut changed = original.clone();
    changed.credential = credential(7);
    requests.push(changed);
    let mut changed = original.clone();
    changed.target_device = device(9);
    requests.push(changed);
    let mut changed = original;
    changed.target_credential = credential(9);
    requests.push(changed);
    for request in requests {
        refuse_revocation(&mut authority(), &sign_revocation(request, &fixture_key()));
    }
    let mut changed = sign_revocation(revocation(), &fixture_key());
    changed.signature[0] ^= 1;
    refuse_revocation(&mut authority(), &changed);
    refuse_revocation(
        &mut authority(),
        &sign_revocation(revocation(), &other_key()),
    );
    let mut changed = sign_revocation(revocation(), &fixture_key());
    changed.request.operation = operation(9);
    refuse_revocation(&mut authority(), &changed);
    let request = revocation();
    let mut wrong_domain = request.canonical_bytes();
    wrong_domain[32] = b'2';
    refuse_revocation(
        &mut authority(),
        &SignedRevocation {
            request,
            signature: fixture_key().sign(&wrong_domain).to_bytes(),
        },
    );
}

#[test]
fn an_administrator_cannot_revoke_another_owners_binding() {
    let mut current = bootstrap(vec![
        known(2, 3, 4, &fixture_key()),
        known(8, 6, 7, &other_key()),
    ])
    .unwrap();
    apply_policy(
        &mut current,
        V2Action::Classic(Action::AddPrincipal {
            principal: principal(8),
        }),
    );
    let mut request = revocation();
    request.base_revision = current.revision();
    refuse_revocation(&mut current, &sign_revocation(request, &fixture_key()));
    assert!(current
        .known_device(&organization(), &principal(8), &device(6), &credential(7))
        .is_ok());
}

#[test]
fn own_revocation_requires_no_enrollment_or_administration_grant() {
    let mut current = bootstrap(vec![
        known(2, 3, 4, &fixture_key()),
        known(8, 6, 7, &other_key()),
        known(8, 9, 10, &third_key()),
    ])
    .unwrap();
    apply_policy(
        &mut current,
        V2Action::Classic(Action::AddPrincipal {
            principal: principal(8),
        }),
    );
    assert!(!current
        .authority
        .can_add_own_device(&organization(), &principal(8)));
    assert!(!current
        .authority
        .can_manage_enrollment_policy(&organization(), &principal(8)));
    let mut request = revocation();
    request.actor = principal(8);
    request.device = device(6);
    request.credential = credential(7);
    request.target_device = device(9);
    request.target_credential = credential(10);
    request.base_revision = current.revision();
    assert_eq!(
        current
            .apply_revocation(&sign_revocation(request, &other_key()))
            .unwrap()
            .revision,
        2
    );
    assert!(!current
        .authority
        .can_add_own_device(&organization(), &principal(8)));
    assert!(!current
        .authority
        .can_manage_enrollment_policy(&organization(), &principal(8)));
}

#[test]
fn self_revocation_can_remove_the_last_device_without_recovery() {
    let mut current = bootstrap(vec![known(2, 3, 4, &fixture_key())]).unwrap();
    let mut request = revocation();
    request.target_device = device(3);
    request.target_credential = credential(4);
    let signed = sign_revocation(request, &fixture_key());
    assert_eq!(current.apply_revocation(&signed).unwrap().revision, 1);
    refuse_revocation(&mut current, &signed);
    refuse_policy(
        &mut current,
        &sign_policy(policy_request(1, 8), &fixture_key()),
    );
    assert!(current
        .known_device(&organization(), &principal(2), &device(3), &credential(4))
        .is_err());
    assert_eq!(current.bindings.len(), 1);
    assert!(current.bindings[0].revoked);
}

#[test]
fn historical_revocation_is_exact_and_does_not_repeat_or_restore_the_target() {
    let mut current = authority();
    let signed = sign_revocation(revocation(), &fixture_key());
    current.apply_revocation(&signed).unwrap();
    apply_policy(&mut current, policy_request(0, 0).action);
    let before = format!("{current:?}");
    assert_eq!(
        current.apply_revocation(&signed),
        Ok(Outcome {
            admission: Admission::Historical,
            revision: 1,
        })
    );
    assert_eq!(format!("{current:?}"), before);
    let mut changed = signed.request.clone();
    changed.base_revision = current.revision();
    refuse_revocation(&mut current, &sign_revocation(changed, &fixture_key()));
    let mut changed = signed.request.clone();
    changed.target_device = device(3);
    changed.target_credential = credential(4);
    refuse_revocation(&mut current, &sign_revocation(changed, &fixture_key()));
    let mut repeated = signed.request.clone();
    repeated.operation = operation(9);
    repeated.base_revision = current.revision();
    refuse_revocation(&mut current, &sign_revocation(repeated, &fixture_key()));
    assert!(current.bindings[1].revoked);
}

#[test]
fn a_surviving_key_cannot_take_over_another_keys_historical_operation() {
    let mut current = bootstrap(vec![
        known(2, 3, 4, &fixture_key()),
        known(2, 6, 7, &other_key()),
        known(2, 9, 10, &third_key()),
    ])
    .unwrap();
    let policy = apply_policy(&mut current, policy_request(0, 0).action);
    let mut request = revocation();
    request.base_revision = current.revision();
    let signed = sign_revocation(request.clone(), &fixture_key());
    current.apply_revocation(&signed).unwrap();
    request.device = device(9);
    request.credential = credential(10);
    refuse_revocation(&mut current, &sign_revocation(request, &third_key()));
    let mut request = policy.request;
    request.device = device(9);
    request.credential = credential(10);
    refuse_policy(&mut current, &sign_policy(request, &third_key()));
    assert_eq!(current.apply_revocation(&signed).unwrap().revision, 2);
}

#[test]
fn revoked_signer_cannot_reconcile_its_old_policy_or_revocation_receipt() {
    let mut current = authority();
    let policy = apply_policy(&mut current, policy_request(0, 0).action);
    let mut request = revocation();
    request.base_revision = current.revision();
    let revocation = sign_revocation(request, &fixture_key());
    current.apply_revocation(&revocation).unwrap();
    let mut request = revocation.request.clone();
    request.operation = operation(6);
    request.base_revision = current.revision();
    request.target_device = device(3);
    request.target_credential = credential(4);
    current
        .apply_revocation(&sign_revocation(request, &fixture_key()))
        .unwrap();
    refuse_policy(&mut current, &policy);
    refuse_revocation(&mut current, &revocation);
}

#[test]
fn principal_revocation_blocks_all_bindings_and_historical_receipts() {
    let mut current = authority();
    let signed = apply_policy(&mut current, policy_request(0, 0).action);
    apply_policy(
        &mut current,
        V2Action::Classic(Action::RevokePrincipal {
            principal: principal(2),
        }),
    );
    refuse_policy(&mut current, &signed);
    let mut request = revocation();
    request.base_revision = current.revision();
    refuse_revocation(&mut current, &sign_revocation(request, &fixture_key()));
    for (device_id, credential_id) in [(3, 4), (6, 7)] {
        assert!(current
            .known_device(
                &organization(),
                &principal(2),
                &device(device_id),
                &credential(credential_id)
            )
            .is_err());
    }
}

#[test]
fn policy_and_revocation_share_revision_and_disallow_cross_type_operations() {
    let mut current = authority();
    let mut policy = policy_request(0, 5);
    policy.action = V2Action::SetOwnAdditionalDevice {
        principal: principal(2),
        enabled: true,
    };
    current
        .apply_signed(&sign_policy(policy, &fixture_key()))
        .unwrap();
    let mut request = revocation();
    request.base_revision = 1;
    refuse_revocation(
        &mut current,
        &sign_revocation(request.clone(), &fixture_key()),
    );
    request.operation = operation(6);
    let mut stale = request.clone();
    stale.base_revision = 0;
    refuse_revocation(&mut current, &sign_revocation(stale, &fixture_key()));
    assert_eq!(
        current
            .apply_revocation(&sign_revocation(request, &fixture_key()))
            .unwrap()
            .revision,
        2
    );
    refuse_policy(
        &mut current,
        &sign_policy(policy_request(2, 6), &fixture_key()),
    );
    refuse_policy(
        &mut current,
        &sign_policy(policy_request(1, 7), &fixture_key()),
    );
    assert_eq!(
        current
            .apply_signed(&sign_policy(policy_request(2, 7), &fixture_key()))
            .unwrap()
            .revision,
        3
    );
}

#[test]
fn surviving_device_can_change_policy_but_cannot_impersonate_revoked_signer() {
    let mut current = authority();
    current
        .apply_revocation(&sign_revocation(revocation(), &fixture_key()))
        .unwrap();
    let mut revoked = policy_request(1, 8);
    revoked.device = device(6);
    revoked.credential = credential(7);
    refuse_policy(&mut current, &sign_policy(revoked.clone(), &other_key()));
    refuse_policy(&mut current, &sign_policy(revoked, &fixture_key()));
    let policy = sign_policy(policy_request(1, 8), &fixture_key());
    assert_eq!(current.apply_signed(&policy).unwrap().revision, 2);
    assert!(current
        .authority
        .can_add_own_device(&organization(), &principal(2)));
    assert!(current.bindings[1].revoked);
}

#[test]
fn mixed_receipts_share_the_operation_bound_and_keep_exact_retries_available() {
    let mut current = authority();
    let revocation = sign_revocation(revocation(), &fixture_key());
    current.apply_revocation(&revocation).unwrap();
    let mut last = None;
    for index in 1..MAX_OPERATIONS {
        let signed = sign_policy(
            policy_request(current.revision(), index as u64 + 100),
            &fixture_key(),
        );
        current.apply_signed(&signed).unwrap();
        last = Some(signed);
    }
    assert_eq!(current.revision(), MAX_OPERATIONS as u64);
    assert_eq!(
        current.authority.receipts.len() + current.revocations.len(),
        MAX_OPERATIONS
    );
    let next = sign_policy(policy_request(current.revision(), 9000), &fixture_key());
    refuse_policy(&mut current, &next);
    let mut request = revocation.request.clone();
    request.base_revision = current.revision();
    request.operation = operation(9001);
    request.target_device = device(3);
    request.target_credential = credential(4);
    refuse_revocation(&mut current, &sign_revocation(request, &fixture_key()));
    assert_eq!(
        current.apply_signed(&last.unwrap()).unwrap().admission,
        Admission::Historical
    );
    assert_eq!(current.apply_revocation(&revocation).unwrap().revision, 1);
    assert_eq!(current.revision(), MAX_OPERATIONS as u64);
}
