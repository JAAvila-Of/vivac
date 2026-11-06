use ed25519_dalek::{Signer, SigningKey};

use super::*;
use crate::team_authority::{
    Action, Admission, OperationId, OrganizationCapability, ProjectId, ProjectPermission,
};

const ORGANIZATION: &str = "00000000000000000000000001";
const PRINCIPAL: &str = "00000000000000000000000002";
const DEVICE: &str = "00000000000000000000000003";
const CREDENTIAL: &str = "00000000000000000000000004";
const OPERATION: &str = "00000000000000000000000005";
const TARGET: &str = "00000000000000000000000006";
const PROJECT: &str = "00000000000000000000000007";
const OTHER: &str = "00000000000000000000000008";

fn key() -> SigningKey {
    // RFC 8032 section 7.1, TEST 1: this is a published test seed.
    SigningKey::from_bytes(&hex(
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    ))
}

fn hex<const N: usize>(text: &str) -> [u8; N] {
    assert_eq!(text.len(), N * 2);
    let mut result = [0; N];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
    }
    result
}

fn request(action: Action) -> DeviceRequest {
    DeviceRequest {
        change: Change {
            organization: OrganizationId::parse(ORGANIZATION).unwrap(),
            actor: PrincipalId::parse(PRINCIPAL).unwrap(),
            operation: OperationId::parse(OPERATION).unwrap(),
            base_revision: 0,
            action,
        },
        device: DeviceId::parse(DEVICE).unwrap(),
        credential: CredentialId::parse(CREDENTIAL).unwrap(),
    }
}

fn known(request: &DeviceRequest, key: &SigningKey) -> KnownDevice {
    KnownDevice::new(
        request.change.organization.clone(),
        request.change.actor.clone(),
        request.device.clone(),
        request.credential.clone(),
        key.verifying_key().to_bytes(),
    )
    .unwrap()
}

fn signed(request: DeviceRequest, key: &SigningKey) -> SignedRequest {
    SignedRequest {
        signature: key.sign(&request.canonical_bytes()).to_bytes(),
        request,
    }
}

fn create_project() -> Action {
    Action::CreateProject {
        project: ProjectId::parse(PROJECT).unwrap(),
    }
}

fn authority() -> SignedAuthority {
    SignedAuthority::bootstrap(
        OrganizationId::parse(ORGANIZATION).unwrap(),
        PrincipalId::parse(PRINCIPAL).unwrap(),
    )
}

fn refused(authority: &mut SignedAuthority, request: &SignedRequest, device: &KnownDevice) {
    let before = authority.clone();
    assert_eq!(authority.apply_signed(request, device), Err(Refused));
    assert_eq!(*authority, before);
}

#[test]
fn published_signature_vector_matches_and_verifies_strictly() {
    let key = key();
    assert_eq!(
        key.verifying_key().to_bytes(),
        hex::<32>("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    );
    let expected = hex::<64>(concat!(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155",
        "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    ));
    assert_eq!(key.sign(b"").to_bytes(), expected);
    assert!(key
        .verifying_key()
        .verify_strict(b"", &Signature::from_bytes(&expected))
        .is_ok());
}

#[test]
fn valid_signature_admits_only_the_exact_authorized_change() {
    let key = key();
    let request = request(create_project());
    let device = known(&request, &key);
    let signed = signed(request.clone(), &key);
    assert_eq!(signed.verify(&device).unwrap(), request.change);
    let mut authority = authority();
    assert_eq!(
        authority.apply_signed(&signed, &device).unwrap(),
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 1,
        }
    );
    assert_eq!(authority.revision(), 1);
    assert!(!authority.allows(
        &request.change.organization,
        &request.change.actor,
        &ProjectId::parse(PROJECT).unwrap(),
        ProjectPermission::ReadContent
    ));
}

#[test]
fn every_action_has_a_fixed_independent_encoding() {
    let header = b"vivac.team.change.v1\0\
        00000000000000000000000001\
        00000000000000000000000002\
        00000000000000000000000003\
        00000000000000000000000004\
        00000000000000000000000005\
        \x01\x02\x03\x04\x05\x06\x07\x08";
    let principal = PrincipalId::parse(TARGET).unwrap();
    let project = ProjectId::parse(PROJECT).unwrap();
    let cases: [(Action, &[u8]); 5] = [
        (
            Action::AddPrincipal {
                principal: principal.clone(),
            },
            b"\x0100000000000000000000000006",
        ),
        (
            Action::SetOrganizationCapabilities {
                principal: principal.clone(),
                capabilities: [
                    OrganizationCapability::ManageMembers,
                    OrganizationCapability::ManageProjects,
                    OrganizationCapability::GrantProjectAccess,
                    OrganizationCapability::ManageOrganizationCapabilities,
                ]
                .into_iter()
                .collect(),
            },
            b"\x0200000000000000000000000006\x0f",
        ),
        (
            Action::CreateProject {
                project: project.clone(),
            },
            b"\x0300000000000000000000000007",
        ),
        (
            Action::SetProjectPermissions {
                principal: principal.clone(),
                project,
                permissions: [
                    ProjectPermission::ReadContent,
                    ProjectPermission::WriteWork,
                    ProjectPermission::ProposeNorm,
                    ProjectPermission::ReadReports,
                    ProjectPermission::ReadAudit,
                ]
                .into_iter()
                .collect(),
            },
            b"\x040000000000000000000000000600000000000000000000000007\x1f",
        ),
        (
            Action::RevokePrincipal { principal },
            b"\x0500000000000000000000000006",
        ),
    ];
    assert_eq!(MAX_ENVELOPE_BYTES, 213);
    for (action, expected) in cases {
        let mut request = request(action);
        request.change.base_revision = 0x0102_0304_0506_0708;
        let bytes = request.canonical_bytes();
        assert_eq!(bytes, [header.as_slice(), expected].concat());
        assert!(bytes.len() <= MAX_ENVELOPE_BYTES);
    }
}

#[test]
fn permission_masks_preserve_each_bit_and_empty_sets() {
    let target = PrincipalId::parse(TARGET).unwrap();
    let cases = [
        (OrganizationCapability::ManageMembers, 1),
        (OrganizationCapability::ManageProjects, 2),
        (OrganizationCapability::GrantProjectAccess, 4),
        (OrganizationCapability::ManageOrganizationCapabilities, 8),
    ];
    for (capability, expected) in cases {
        let bytes = request(Action::SetOrganizationCapabilities {
            principal: target.clone(),
            capabilities: [capability].into_iter().collect(),
        })
        .canonical_bytes();
        assert_eq!(bytes.last(), Some(&expected));
    }
    let cases = [
        (ProjectPermission::ReadContent, 1),
        (ProjectPermission::WriteWork, 2),
        (ProjectPermission::ProposeNorm, 4),
        (ProjectPermission::ReadReports, 8),
        (ProjectPermission::ReadAudit, 16),
    ];
    for (permission, expected) in cases {
        let bytes = request(Action::SetProjectPermissions {
            principal: target.clone(),
            project: ProjectId::parse(PROJECT).unwrap(),
            permissions: [permission].into_iter().collect(),
        })
        .canonical_bytes();
        assert_eq!(bytes.len(), MAX_ENVELOPE_BYTES);
        assert_eq!(bytes.last(), Some(&expected));
    }
    for action in [
        Action::SetOrganizationCapabilities {
            principal: target.clone(),
            capabilities: Default::default(),
        },
        Action::SetProjectPermissions {
            principal: target,
            project: ProjectId::parse(PROJECT).unwrap(),
            permissions: Default::default(),
        },
    ] {
        assert_eq!(request(action).canonical_bytes().last(), Some(&0));
    }
}

#[test]
fn every_changed_envelope_byte_and_signature_byte_refuses_without_mutation() {
    let key = key();
    let request = request(Action::SetProjectPermissions {
        principal: PrincipalId::parse(TARGET).unwrap(),
        project: ProjectId::parse(PROJECT).unwrap(),
        permissions: [ProjectPermission::ReadContent].into_iter().collect(),
    });
    let device = known(&request, &key);
    let original = signed(request, &key);
    let mut authority = authority();
    let bytes = original.request.canonical_bytes();
    for index in 0..bytes.len() {
        let mut changed = bytes.clone();
        changed[index] ^= 1;
        let mut candidate = original.clone();
        candidate.signature = key.sign(&changed).to_bytes();
        assert_eq!(candidate.verify(&device), Err(Refused));
        refused(&mut authority, &candidate, &device);
    }
    for index in 0..original.signature.len() {
        let mut candidate = original.clone();
        candidate.signature[index] ^= 1;
        assert_eq!(candidate.verify(&device), Err(Refused));
        refused(&mut authority, &candidate, &device);
    }
}

#[test]
fn every_changed_request_field_is_covered_by_the_signature() {
    let key = key();
    let original = signed(
        request(Action::SetProjectPermissions {
            principal: PrincipalId::parse(TARGET).unwrap(),
            project: ProjectId::parse(PROJECT).unwrap(),
            permissions: [ProjectPermission::ReadContent].into_iter().collect(),
        }),
        &key,
    );
    let mut candidates = vec![original.clone(); 9];
    candidates[0].request.change.organization = OrganizationId::parse(OTHER).unwrap();
    candidates[1].request.change.actor = PrincipalId::parse(OTHER).unwrap();
    candidates[2].request.device = DeviceId::parse(OTHER).unwrap();
    candidates[3].request.credential = CredentialId::parse(OTHER).unwrap();
    candidates[4].request.change.operation = OperationId::parse(OTHER).unwrap();
    candidates[5].request.change.base_revision = 1;
    candidates[6].request.change.action = Action::SetProjectPermissions {
        principal: PrincipalId::parse(OTHER).unwrap(),
        project: ProjectId::parse(PROJECT).unwrap(),
        permissions: [ProjectPermission::ReadContent].into_iter().collect(),
    };
    candidates[7].request.change.action = Action::SetProjectPermissions {
        principal: PrincipalId::parse(TARGET).unwrap(),
        project: ProjectId::parse(OTHER).unwrap(),
        permissions: [ProjectPermission::ReadContent].into_iter().collect(),
    };
    candidates[8].request.change.action = Action::SetProjectPermissions {
        principal: PrincipalId::parse(TARGET).unwrap(),
        project: ProjectId::parse(PROJECT).unwrap(),
        permissions: [ProjectPermission::WriteWork].into_iter().collect(),
    };
    for action in [
        Action::AddPrincipal {
            principal: PrincipalId::parse(TARGET).unwrap(),
        },
        Action::SetOrganizationCapabilities {
            principal: PrincipalId::parse(TARGET).unwrap(),
            capabilities: [OrganizationCapability::ManageMembers]
                .into_iter()
                .collect(),
        },
        create_project(),
        Action::RevokePrincipal {
            principal: PrincipalId::parse(TARGET).unwrap(),
        },
    ] {
        let mut candidate = original.clone();
        candidate.request.change.action = action;
        candidates.push(candidate);
    }
    let mut authority = authority();
    for candidate in candidates {
        let device = known(&candidate.request, &key);
        assert_eq!(candidate.verify(&device), Err(Refused));
        refused(&mut authority, &candidate, &device);
    }
}

#[test]
fn matching_signature_cannot_substitute_any_trusted_binding_field() {
    let key = key();
    let original = signed(request(create_project()), &key);
    let device = known(&original.request, &key);
    let mut bindings = vec![device.clone(); 5];
    bindings[0].organization = OrganizationId::parse(OTHER).unwrap();
    bindings[1].principal = PrincipalId::parse(OTHER).unwrap();
    bindings[2].device = DeviceId::parse(OTHER).unwrap();
    bindings[3].credential = CredentialId::parse(OTHER).unwrap();
    bindings[4].key = SigningKey::from_bytes(&[42; 32]).verifying_key();
    let mut authority = authority();
    for binding in bindings {
        assert_eq!(original.verify(&binding), Err(Refused));
        refused(&mut authority, &original, &binding);
    }
    let mut foreign = original.request;
    foreign.change.organization = OrganizationId::parse(OTHER).unwrap();
    let foreign_device = known(&foreign, &key);
    let foreign = signed(foreign, &key);
    assert!(foreign.verify(&foreign_device).is_ok());
    refused(&mut authority, &foreign, &foreign_device);
}

#[test]
fn known_devices_refuse_malformed_and_weak_public_keys() {
    let request = request(create_project());
    let mut identity = [0; 32];
    identity[0] = 1;
    for key in [[0; 32], identity] {
        assert!(VerifyingKey::from_bytes(&key).unwrap().is_weak());
        assert!(KnownDevice::new(
            request.change.organization.clone(),
            request.change.actor.clone(),
            request.device.clone(),
            request.credential.clone(),
            key,
        )
        .is_err());
    }
    let malformed = [2; 32];
    assert!(VerifyingKey::from_bytes(&malformed).is_err());
    assert!(KnownDevice::new(
        request.change.organization,
        request.change.actor,
        request.device,
        request.credential,
        malformed,
    )
    .is_err());
}

#[test]
fn canonical_identifiers_are_required_for_devices_and_credentials() {
    for value in [
        "",
        "short",
        "0000000000000000000000000A",
        "80000000000000000000000000",
    ] {
        assert!(DeviceId::parse(value).is_err());
        assert!(CredentialId::parse(value).is_err());
    }
    assert!(DeviceId::parse(DEVICE).is_ok());
    assert!(CredentialId::parse(CREDENTIAL).is_ok());
}

#[test]
fn exact_retry_is_historical_but_changed_signed_envelope_is_not() {
    let key = key();
    let original = signed(request(create_project()), &key);
    let device = known(&original.request, &key);
    let mut authority = authority();
    let first = authority.apply_signed(&original, &device).unwrap();
    let before = authority.clone();
    assert_eq!(
        authority.apply_signed(&original, &device).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: first.revision,
        }
    );
    assert_eq!(authority, before);
    let mut candidates = vec![original.request.clone(); 4];
    candidates[0].device = DeviceId::parse(OTHER).unwrap();
    candidates[1].credential = CredentialId::parse(OTHER).unwrap();
    candidates[2].change.action = Action::CreateProject {
        project: ProjectId::parse(OTHER).unwrap(),
    };
    candidates[3].change.base_revision = 1;
    for candidate in candidates {
        let binding = known(&candidate, &key);
        let candidate = signed(candidate, &key);
        assert!(candidate.verify(&binding).is_ok());
        refused(&mut authority, &candidate, &binding);
    }
    let replacement = SigningKey::from_bytes(&[42; 32]);
    let replacement_binding = known(&original.request, &replacement);
    let replacement_request = signed(original.request, &replacement);
    assert!(replacement_request.verify(&replacement_binding).is_ok());
    refused(&mut authority, &replacement_request, &replacement_binding);
}

fn admit(
    authority: &mut SignedAuthority,
    action: Action,
    operation: u64,
    key: &SigningKey,
) -> SignedRequest {
    let mut request = request(action);
    request.change.operation = OperationId::parse(&format!("{operation:026x}")).unwrap();
    request.change.base_revision = authority.revision();
    let device = known(&request, key);
    let signed = signed(request, key);
    assert_eq!(
        authority.apply_signed(&signed, &device).unwrap().admission,
        Admission::NewlyAdmitted
    );
    signed
}

#[test]
fn valid_signatures_do_not_bypass_permissions_or_stale_revisions() {
    let key = key();
    let mut authority = authority();
    admit(&mut authority, create_project(), 10, &key);
    let stale = signed(
        request(Action::AddPrincipal {
            principal: PrincipalId::parse(TARGET).unwrap(),
        }),
        &key,
    );
    let stale_device = known(&stale.request, &key);
    assert!(stale.verify(&stale_device).is_ok());
    refused(&mut authority, &stale, &stale_device);
    admit(
        &mut authority,
        Action::AddPrincipal {
            principal: PrincipalId::parse(TARGET).unwrap(),
        },
        11,
        &key,
    );
    let mut unauthorized = request(Action::CreateProject {
        project: ProjectId::parse(OTHER).unwrap(),
    });
    unauthorized.change.base_revision = authority.revision();
    unauthorized.change.actor = PrincipalId::parse(TARGET).unwrap();
    let unauthorized_device = known(&unauthorized, &key);
    let unauthorized = signed(unauthorized, &key);
    assert!(unauthorized.verify(&unauthorized_device).is_ok());
    refused(&mut authority, &unauthorized, &unauthorized_device);
}

#[test]
fn revoked_principals_cannot_submit_or_reconcile_a_valid_signature() {
    let key = key();
    let mut authority = authority();
    let original = admit(&mut authority, create_project(), 10, &key);
    let device = known(&original.request, &key);
    admit(
        &mut authority,
        Action::RevokePrincipal {
            principal: PrincipalId::parse(PRINCIPAL).unwrap(),
        },
        11,
        &key,
    );
    assert!(original.verify(&device).is_ok());
    refused(&mut authority, &original, &device);
    let mut fresh = request(Action::CreateProject {
        project: ProjectId::parse(OTHER).unwrap(),
    });
    fresh.change.base_revision = authority.revision();
    let fresh = signed(fresh, &key);
    assert!(fresh.verify(&device).is_ok());
    refused(&mut authority, &fresh, &device);
}

#[test]
fn signed_receipts_stop_at_the_policy_bound_without_evicting_history() {
    let key = key();
    let mut authority = authority();
    let original = admit(&mut authority, create_project(), 10, &key);
    let target = PrincipalId::parse(PRINCIPAL).unwrap();
    for index in 1..MAX_OPERATIONS {
        admit(
            &mut authority,
            Action::SetProjectPermissions {
                principal: target.clone(),
                project: ProjectId::parse(PROJECT).unwrap(),
                permissions: Default::default(),
            },
            10 + index as u64,
            &key,
        );
    }
    assert_eq!(authority.receipts.len(), MAX_OPERATIONS);
    assert_eq!(authority.authority.receipts.len(), MAX_OPERATIONS);
    let mut next = request(Action::SetProjectPermissions {
        principal: target,
        project: ProjectId::parse(PROJECT).unwrap(),
        permissions: [ProjectPermission::ReadContent].into_iter().collect(),
    });
    next.change.base_revision = authority.revision();
    next.change.operation = OperationId::parse(OTHER).unwrap();
    let device = known(&next, &key);
    refused(&mut authority, &signed(next, &key), &device);
    let before = authority.clone();
    assert_eq!(
        authority
            .apply_signed(&original, &device)
            .unwrap()
            .admission,
        Admission::Historical
    );
    assert_eq!(authority, before);
}
