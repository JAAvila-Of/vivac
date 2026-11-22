use ed25519_dalek::{Signer, SigningKey};

use super::*;

fn organization() -> OrganizationId {
    OrganizationId::parse("00000000000000000000000001").unwrap()
}

fn principal(value: u64) -> PrincipalId {
    PrincipalId::parse(&format!("{value:026}")).unwrap()
}

fn fixture_key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 1: this is a published test seed.
    SigningKey::from_bytes(&[
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ])
}

fn authority() -> SignedAuthorityV2 {
    SignedAuthorityV2::bootstrap(organization(), principal(2), principal(2)).unwrap()
}

fn grant(target: u64, enabled: bool) -> V2Action {
    V2Action::SetOwnAdditionalDevice {
        principal: principal(target),
        enabled,
    }
}

fn administrator(target: u64, enabled: bool) -> V2Action {
    V2Action::SetEnrollmentPolicyAdministrator {
        principal: principal(target),
        enabled,
    }
}

fn request(actor: u64, revision: u64, action: V2Action) -> DeviceRequestV2 {
    DeviceRequestV2 {
        organization: organization(),
        actor: principal(actor),
        device: DeviceId::parse("00000000000000000000000003").unwrap(),
        credential: CredentialId::parse("00000000000000000000000004").unwrap(),
        operation: OperationId::parse(&format!("{:026}", revision + 100)).unwrap(),
        base_revision: revision,
        action,
    }
}

fn sign(request: DeviceRequestV2, key: &SigningKey) -> SignedRequestV2 {
    SignedRequestV2 {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    }
}

fn known(request: &DeviceRequestV2, key: &SigningKey) -> KnownDevice {
    KnownDevice::new(
        request.organization.clone(),
        request.actor.clone(),
        request.device.clone(),
        request.credential.clone(),
        key.verifying_key().to_bytes(),
    )
    .unwrap()
}

fn admit(current: &mut SignedAuthorityV2, actor: u64, action: V2Action) -> SignedRequestV2 {
    let request = request(actor, current.revision(), action);
    let key = fixture_key();
    let device = known(&request, &key);
    let signed = sign(request, &key);
    let expected = current.revision() + 1;
    assert_eq!(
        current.apply_signed(&signed, &device),
        Ok(Outcome {
            admission: Admission::NewlyAdmitted,
            revision: expected,
        })
    );
    signed
}

fn refuse(current: &mut SignedAuthorityV2, signed: &SignedRequestV2, device: &KnownDevice) {
    let before = current.clone();
    assert_eq!(current.apply_signed(signed, device), Err(Refused));
    assert_eq!(*current, before);
}

fn refuse_action(current: &mut SignedAuthorityV2, actor: u64, action: V2Action) {
    let request = request(actor, current.revision(), action);
    let key = fixture_key();
    let device = known(&request, &key);
    refuse(current, &sign(request, &key), &device);
}

fn add_principal(current: &mut SignedAuthorityV2, target: u64) {
    admit(
        current,
        2,
        V2Action::Classic(Action::AddPrincipal {
            principal: principal(target),
        }),
    );
}

fn capabilities(current: &mut SignedAuthorityV2, target: u64, values: &[OrganizationCapability]) {
    admit(
        current,
        2,
        V2Action::Classic(Action::SetOrganizationCapabilities {
            principal: principal(target),
            capabilities: values.iter().copied().collect(),
        }),
    );
}

#[test]
fn a_policy_administrator_can_grant_own_additional_device_explicitly() {
    let organization = OrganizationId::parse("00000000000000000000000001").unwrap();
    let principal = PrincipalId::parse("00000000000000000000000002").unwrap();
    // RFC 8032 section 7.1, TEST 1: this is a published test seed.
    let key = SigningKey::from_bytes(&[
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ]);
    let mut authority =
        SignedAuthorityV2::bootstrap(organization.clone(), principal.clone(), principal.clone())
            .unwrap();
    assert!(!authority.can_add_own_device(&organization, &principal));
    let request = DeviceRequestV2 {
        organization: organization.clone(),
        actor: principal.clone(),
        device: DeviceId::parse("00000000000000000000000003").unwrap(),
        credential: CredentialId::parse("00000000000000000000000004").unwrap(),
        operation: OperationId::parse("00000000000000000000000005").unwrap(),
        base_revision: 0,
        action: V2Action::SetOwnAdditionalDevice {
            principal: principal.clone(),
            enabled: true,
        },
    };
    let known = KnownDevice::new(
        organization.clone(),
        principal.clone(),
        request.device.clone(),
        request.credential.clone(),
        key.verifying_key().to_bytes(),
    )
    .unwrap();
    let signed = SignedRequestV2 {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    };
    assert_eq!(
        authority
            .apply_signed(&signed, &known)
            .expect("explicit grant"),
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 1,
        }
    );
    assert!(authority.can_add_own_device(&organization, &principal));
}

#[test]
fn bootstrap_requires_the_explicit_initial_administrator_without_an_own_grant() {
    assert_eq!(
        SignedAuthorityV2::bootstrap(organization(), principal(2), principal(6)),
        Err(Refused)
    );
    let current = authority();
    assert!(current.can_manage_enrollment_policy(&organization(), &principal(2)));
    assert!(!current.can_add_own_device(&organization(), &principal(2)));
    for target in [principal(6), principal(9)] {
        assert!(!current.can_manage_enrollment_policy(&organization(), &target));
        assert!(!current.can_add_own_device(&organization(), &target));
    }
    let other = OrganizationId::parse("00000000000000000000000009").unwrap();
    assert!(!current.can_manage_enrollment_policy(&other, &principal(2)));
    assert!(!current.can_add_own_device(&other, &principal(2)));
}

#[test]
fn membership_capabilities_and_own_grants_do_not_convey_policy_administration() {
    let mut current = authority();
    add_principal(&mut current, 6);
    capabilities(
        &mut current,
        6,
        &[
            OrganizationCapability::ManageMembers,
            OrganizationCapability::ManageOrganizationCapabilities,
        ],
    );
    assert!(!current.can_add_own_device(&organization(), &principal(6)));
    assert!(!current.can_manage_enrollment_policy(&organization(), &principal(6)));
    refuse_action(&mut current, 6, grant(6, true));
    refuse_action(&mut current, 6, administrator(6, true));
    admit(&mut current, 2, grant(6, true));
    assert!(current.can_add_own_device(&organization(), &principal(6)));
    refuse_action(&mut current, 6, grant(2, true));
    refuse_action(&mut current, 6, administrator(6, true));
    admit(&mut current, 2, grant(6, false));
    assert!(!current.can_add_own_device(&organization(), &principal(6)));
}

#[test]
fn a_delegated_administrator_needs_classic_governance_to_change_administrators() {
    let mut current = authority();
    add_principal(&mut current, 6);
    admit(&mut current, 2, administrator(6, true));
    assert!(current.can_manage_enrollment_policy(&organization(), &principal(6)));
    admit(&mut current, 6, grant(2, true));
    admit(&mut current, 6, grant(2, false));
    refuse_action(&mut current, 6, administrator(6, false));
    refuse_action(&mut current, 6, administrator(2, false));
    refuse_action(&mut current, 6, administrator(9, true));
    capabilities(
        &mut current,
        6,
        &[OrganizationCapability::ManageOrganizationCapabilities],
    );
    admit(&mut current, 6, administrator(2, false));
    assert!(!current.can_manage_enrollment_policy(&organization(), &principal(2)));
    refuse_action(&mut current, 2, grant(2, true));
    admit(&mut current, 6, administrator(6, false));
    assert!(current.policy_admins.is_empty());
    refuse_action(&mut current, 2, administrator(2, true));
    refuse_action(&mut current, 6, administrator(6, true));
    refuse_action(&mut current, 6, grant(6, true));
}

#[test]
fn revocation_clears_both_policy_powers_and_inactive_actors_cannot_retry() {
    let mut current = authority();
    add_principal(&mut current, 6);
    admit(&mut current, 2, administrator(6, true));
    let granted = admit(&mut current, 6, grant(6, true));
    admit(
        &mut current,
        2,
        V2Action::Classic(Action::RevokePrincipal {
            principal: principal(6),
        }),
    );
    assert!(!current.policy_admins.contains(&principal(6)));
    assert!(!current.own_grants.contains(&principal(6)));
    assert!(!current.can_manage_enrollment_policy(&organization(), &principal(6)));
    assert!(!current.can_add_own_device(&organization(), &principal(6)));
    refuse(
        &mut current,
        &granted,
        &known(&granted.request, &fixture_key()),
    );
    refuse_action(&mut current, 2, grant(6, true));
    refuse_action(&mut current, 2, administrator(6, true));
    refuse_action(&mut current, 2, grant(9, true));
    refuse_action(&mut current, 2, administrator(9, true));
}

#[test]
fn classic_and_policy_actions_share_order_and_historical_retries_do_not_restore_grants() {
    let mut current = authority();
    let stale = sign(
        request(
            2,
            0,
            V2Action::Classic(Action::AddPrincipal {
                principal: principal(6),
            }),
        ),
        &fixture_key(),
    );
    let granted = admit(&mut current, 2, grant(2, true));
    assert!(current.core.receipts.is_empty());
    refuse(&mut current, &stale, &known(&stale.request, &fixture_key()));
    add_principal(&mut current, 6);
    assert_eq!(current.core.receipts.len(), 1);
    admit(&mut current, 2, grant(2, false));
    let before = current.clone();
    assert_eq!(
        current.apply_signed(&granted, &known(&granted.request, &fixture_key())),
        Ok(Outcome {
            admission: Admission::Historical,
            revision: 1,
        })
    );
    assert_eq!(current, before);
    assert!(!current.can_add_own_device(&organization(), &principal(2)));
    let mut different = granted.request.clone();
    different.action = grant(6, true);
    let different = sign(different, &fixture_key());
    refuse(
        &mut current,
        &different,
        &known(&different.request, &fixture_key()),
    );
    let mut different = granted.request.clone();
    different.action = V2Action::Classic(Action::CreateProject {
        project: ProjectId::parse("00000000000000000000000007").unwrap(),
    });
    let different = sign(different, &fixture_key());
    refuse(
        &mut current,
        &different,
        &known(&different.request, &fixture_key()),
    );
}

#[test]
fn historical_policy_receipts_do_not_reauthorize_a_former_administrator() {
    let mut current = authority();
    add_principal(&mut current, 6);
    admit(&mut current, 2, administrator(6, true));
    let granted = admit(&mut current, 6, grant(6, true));
    admit(&mut current, 2, administrator(6, false));
    admit(&mut current, 2, grant(6, false));
    let before = current.clone();
    assert_eq!(
        current.apply_signed(&granted, &known(&granted.request, &fixture_key())),
        Ok(Outcome {
            admission: Admission::Historical,
            revision: 3,
        })
    );
    assert_eq!(current, before);
    assert!(!current.can_add_own_device(&organization(), &principal(6)));
    refuse_action(&mut current, 6, grant(6, true));
}

#[test]
fn classic_project_authority_keeps_its_rules_and_uses_the_shared_revision() {
    let mut current = authority();
    add_principal(&mut current, 6);
    let project = ProjectId::parse("00000000000000000000000007").unwrap();
    refuse_action(
        &mut current,
        6,
        V2Action::Classic(Action::CreateProject {
            project: project.clone(),
        }),
    );
    admit(&mut current, 2, grant(6, true));
    admit(
        &mut current,
        2,
        V2Action::Classic(Action::CreateProject {
            project: project.clone(),
        }),
    );
    admit(
        &mut current,
        2,
        V2Action::Classic(Action::SetProjectPermissions {
            principal: principal(6),
            project: project.clone(),
            permissions: [ProjectPermission::ReadContent].into_iter().collect(),
        }),
    );
    assert!(current.allows(
        &organization(),
        &principal(6),
        &project,
        ProjectPermission::ReadContent
    ));
    assert!(!current.allows(
        &organization(),
        &principal(6),
        &project,
        ProjectPermission::WriteWork
    ));
    let classic = admit(
        &mut current,
        2,
        V2Action::Classic(Action::RevokePrincipal {
            principal: principal(6),
        }),
    );
    assert!(!current.allows(
        &organization(),
        &principal(6),
        &project,
        ProjectPermission::ReadContent
    ));
    assert_eq!(
        current.apply_signed(&classic, &known(&classic.request, &fixture_key())),
        Ok(Outcome {
            admission: Admission::Historical,
            revision: 5,
        })
    );
}

#[test]
fn signatures_bind_all_header_fields_and_the_exact_trusted_key_before_history() {
    let mut current = authority();
    let original = admit(&mut current, 2, grant(2, true));
    let key = fixture_key();
    let device = known(&original.request, &key);
    let other_key = SigningKey::from_bytes(&[42; 32]);
    let replacement = known(&original.request, &other_key);
    refuse(&mut current, &original, &replacement);
    refuse(
        &mut current,
        &sign(original.request.clone(), &other_key),
        &replacement,
    );
    for field in 0..7 {
        let mut altered = original.request.clone();
        match field {
            0 => {
                altered.organization = OrganizationId::parse("00000000000000000000000009").unwrap()
            }
            1 => altered.actor = principal(9),
            2 => altered.device = DeviceId::parse("00000000000000000000000009").unwrap(),
            3 => altered.credential = CredentialId::parse("00000000000000000000000009").unwrap(),
            4 => altered.operation = OperationId::parse("00000000000000000000000009").unwrap(),
            5 => altered.base_revision += 1,
            _ => altered.action = grant(2, false),
        }
        let tampered = SignedRequestV2 {
            request: altered.clone(),
            signature: original.signature,
        };
        refuse(&mut current, &tampered, &device);
        if field <= 3 {
            let signed = sign(altered.clone(), &key);
            refuse(&mut current, &signed, &device);
            refuse(&mut current, &signed, &known(&altered, &key));
        }
    }
    let mut bad_signature = original.clone();
    bad_signature.signature[63] ^= 0x80;
    refuse(&mut current, &bad_signature, &device);
    let mut future = request(2, current.revision() + 1, grant(2, false));
    future.operation = OperationId::parse("00000000000000000000000009").unwrap();
    let signed = sign(future, &key);
    refuse(&mut current, &signed, &device);
}

#[test]
fn receipt_capacity_covers_classic_and_policy_actions_but_preserves_exact_retries() {
    let mut current = authority();
    let first = admit(&mut current, 2, grant(2, true));
    for index in 1..MAX_OPERATIONS {
        if index % 2 == 0 {
            admit(&mut current, 2, grant(2, index % 4 == 0));
        } else {
            capabilities(
                &mut current,
                2,
                &[
                    OrganizationCapability::ManageMembers,
                    OrganizationCapability::ManageOrganizationCapabilities,
                ],
            );
        }
    }
    assert_eq!(current.revision(), 1024);
    assert_eq!(current.receipts.len(), MAX_OPERATIONS);
    assert_eq!(current.core.receipts.len(), MAX_OPERATIONS / 2);
    refuse_action(&mut current, 2, grant(2, false));
    refuse_action(
        &mut current,
        2,
        V2Action::Classic(Action::AddPrincipal {
            principal: principal(6),
        }),
    );
    let before = current.clone();
    assert_eq!(
        current.apply_signed(&first, &known(&first.request, &fixture_key())),
        Ok(Outcome {
            admission: Admission::Historical,
            revision: 1,
        })
    );
    assert_eq!(current, before);
}

#[test]
fn revision_overflow_refuses_without_changing_policy_or_classic_state() {
    let mut current = authority();
    current.core.revision = u64::MAX;
    for action in [
        grant(2, true),
        administrator(2, false),
        V2Action::Classic(Action::AddPrincipal {
            principal: principal(6),
        }),
    ] {
        let mut request = request(2, 0, action);
        request.base_revision = u64::MAX;
        let signed = sign(request, &fixture_key());
        refuse(
            &mut current,
            &signed,
            &known(&signed.request, &fixture_key()),
        );
    }
}

#[test]
fn canonical_policy_bytes_match_independent_golden_hashes() {
    use sha2::{Digest, Sha256};
    for (action, tag, expected) in [
        (
            grant(6, true),
            6,
            "bed28f1fb7a7855f498a06748885f6d878594f8258dbaff4cb702af56e82486b",
        ),
        (
            administrator(6, true),
            7,
            "7ca9d39fc62d64e5f5920b842902a6e0d63db80a8c40a580f81611537002744b",
        ),
    ] {
        let mut request = request(2, 0, action);
        request.operation = OperationId::parse("00000000000000000000000005").unwrap();
        request.base_revision = 0x0102030405060708;
        let bytes = request.canonical_bytes();
        let mut expected_bytes = b"vivac.team.change.v2\0".to_vec();
        for value in 1..=5 {
            expected_bytes.extend_from_slice(format!("{value:026}").as_bytes());
        }
        expected_bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, tag]);
        expected_bytes.extend_from_slice(b"00000000000000000000000006");
        expected_bytes.push(1);
        assert_eq!(bytes, expected_bytes);
        assert_eq!(bytes.len(), 187);
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), expected);
        assert_eq!(DeviceRequestV2::parse_canonical(&bytes), Ok(request));
    }
}

#[test]
fn the_codec_closes_every_tag_mask_identifier_boolean_and_message_boundary() {
    let actions = [
        V2Action::Classic(Action::AddPrincipal {
            principal: principal(6),
        }),
        V2Action::Classic(Action::SetOrganizationCapabilities {
            principal: principal(6),
            capabilities: [
                OrganizationCapability::ManageMembers,
                OrganizationCapability::ManageProjects,
                OrganizationCapability::GrantProjectAccess,
                OrganizationCapability::ManageOrganizationCapabilities,
            ]
            .into_iter()
            .collect(),
        }),
        V2Action::Classic(Action::CreateProject {
            project: ProjectId::parse("00000000000000000000000007").unwrap(),
        }),
        V2Action::Classic(Action::SetProjectPermissions {
            principal: principal(6),
            project: ProjectId::parse("00000000000000000000000007").unwrap(),
            permissions: [
                ProjectPermission::ReadContent,
                ProjectPermission::WriteWork,
                ProjectPermission::ProposeNorm,
                ProjectPermission::ReadReports,
                ProjectPermission::ReadAudit,
            ]
            .into_iter()
            .collect(),
        }),
        V2Action::Classic(Action::RevokePrincipal {
            principal: principal(6),
        }),
        grant(6, false),
        grant(6, true),
        administrator(6, false),
        administrator(6, true),
    ];
    for action in actions {
        let request = request(2, 0, action);
        let bytes = request.canonical_bytes();
        assert!(bytes.len() <= MAX_ENVELOPE_BYTES);
        assert_eq!(
            DeviceRequestV2::parse_canonical(&bytes),
            Ok(request.clone())
        );
        for length in 0..bytes.len() {
            assert_eq!(
                DeviceRequestV2::parse_canonical(&bytes[..length]),
                Err(Refused)
            );
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(DeviceRequestV2::parse_canonical(&extra), Err(Refused));
        for offset in [
            DOMAIN.len(),
            DOMAIN.len() + 26,
            DOMAIN.len() + 52,
            DOMAIN.len() + 78,
            DOMAIN.len() + 104,
            TAG_OFFSET + 1,
        ] {
            for invalid in [b'8', b'A', b'i', 0xff] {
                let mut altered = bytes.clone();
                altered[offset] = invalid;
                assert_eq!(DeviceRequestV2::parse_canonical(&altered), Err(Refused));
            }
        }
        for tag in [0, 8, 255] {
            let mut altered = bytes.clone();
            altered[TAG_OFFSET] = tag;
            assert_eq!(DeviceRequestV2::parse_canonical(&altered), Err(Refused));
        }
        let tag = bytes[TAG_OFFSET];
        if matches!(tag, 2 | 4 | 6 | 7) {
            let maximum = match tag {
                2 => 15,
                4 => 31,
                _ => 1,
            };
            for invalid in maximum + 1..=255 {
                let mut altered = bytes.clone();
                *altered.last_mut().unwrap() = invalid;
                assert_eq!(DeviceRequestV2::parse_canonical(&altered), Err(Refused));
            }
        }
        assert_eq!(DeviceRequest::parse_canonical(&bytes), Err(Refused));
        if let V2Action::Classic(action) = &request.action {
            let classic = request.classic_request(action.clone()).canonical_bytes();
            assert_eq!(DeviceRequestV2::parse_canonical(&classic), Err(Refused));
            assert_eq!(
                &bytes[DOMAIN.len()..],
                &classic[super::super::DOMAIN.len()..]
            );
        }
    }
    assert_eq!(
        DeviceRequestV2::parse_canonical(&vec![0; MAX_ENVELOPE_BYTES + 1]),
        Err(Refused)
    );
}

#[test]
fn signatures_cannot_cross_protocol_domains_in_either_direction() {
    let request = request(
        2,
        0,
        V2Action::Classic(Action::CreateProject {
            project: ProjectId::parse("00000000000000000000000007").unwrap(),
        }),
    );
    let V2Action::Classic(action) = request.action.clone() else {
        unreachable!()
    };
    let classic = request.classic_request(action);
    let key = fixture_key();
    let device = known(&request, &key);
    let v1 = SignedRequest {
        signature: key.sign(&classic.canonical_bytes()).to_bytes(),
        request: classic.clone(),
    };
    let v2 = sign(request.clone(), &key);
    assert!(v1.verify(&device).is_ok());
    assert!(v2.verify(&device).is_ok());
    let wrong_v2 = SignedRequestV2 {
        request,
        signature: v1.signature,
    };
    let wrong_v1 = SignedRequest {
        request: classic,
        signature: v2.signature,
    };
    assert_eq!(wrong_v2.verify(&device), Err(Refused));
    assert_eq!(wrong_v1.verify(&device), Err(Refused));
}

#[test]
fn weak_device_keys_and_noncanonical_signatures_refuse() {
    let request = request(2, 0, grant(2, true));
    let key = fixture_key();
    let device = known(&request, &key);
    let mut signed = sign(request.clone(), &key);
    assert!(signed.verify(&device).is_ok());
    signed.signature[32..].fill(0xff);
    let mut current = authority();
    refuse(&mut current, &signed, &device);
    let mut identity = [0; 32];
    identity[0] = 1;
    assert!(KnownDevice::new(
        request.organization,
        request.actor,
        request.device,
        request.credential,
        identity,
    )
    .is_err());
}
