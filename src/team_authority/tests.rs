use super::*;

fn synthetic(value: u64) -> String {
    format!("{value:026x}")
}

#[test]
fn explicit_manager_can_add_a_principal() {
    let organization = OrganizationId::parse(&synthetic(1)).unwrap();
    let initial = PrincipalId::parse(&synthetic(10)).unwrap();
    let target = PrincipalId::parse(&synthetic(11)).unwrap();
    let mut authority = Authority::bootstrap(organization.clone(), initial.clone());
    let outcome = authority
        .apply(Change {
            organization,
            operation: OperationId::parse(&synthetic(100)).unwrap(),
            base_revision: 0,
            actor: initial,
            action: Action::AddPrincipal { principal: target },
        })
        .unwrap();
    assert_eq!(
        outcome,
        Outcome {
            admission: Admission::NewlyAdmitted,
            revision: 1
        }
    );
}

const CAPABILITIES: [OrganizationCapability; 4] = [
    OrganizationCapability::ManageMembers,
    OrganizationCapability::ManageProjects,
    OrganizationCapability::GrantProjectAccess,
    OrganizationCapability::ManageOrganizationCapabilities,
];
const PERMISSIONS: [ProjectPermission; 5] = [
    ProjectPermission::ReadContent,
    ProjectPermission::WriteWork,
    ProjectPermission::ProposeNorm,
    ProjectPermission::ReadReports,
    ProjectPermission::ReadAudit,
];

struct Fixture {
    authority: Authority,
    organization: OrganizationId,
    other_organization: OrganizationId,
    principals: [PrincipalId; 4],
    projects: [ProjectId; 2],
    next_operation: u64,
}

impl Fixture {
    fn new() -> Self {
        let organization = OrganizationId::parse(&synthetic(1)).unwrap();
        let principals =
            [10, 11, 12, 13].map(|value| PrincipalId::parse(&synthetic(value)).unwrap());
        let mut fixture = Self {
            authority: Authority::bootstrap(organization.clone(), principals[0].clone()),
            organization,
            other_organization: OrganizationId::parse(&synthetic(2)).unwrap(),
            principals,
            projects: [20, 21].map(|value| ProjectId::parse(&synthetic(value)).unwrap()),
            next_operation: 100,
        };
        for principal in fixture.principals.clone().into_iter().skip(1) {
            fixture.admit(0, Action::AddPrincipal { principal });
        }
        for project in fixture.projects.clone() {
            fixture.admit(0, Action::CreateProject { project });
        }
        fixture
    }

    fn change(&mut self, actor: usize, action: Action) -> Change {
        let operation = OperationId::parse(&synthetic(self.next_operation)).unwrap();
        self.next_operation += 1;
        Change {
            organization: self.organization.clone(),
            operation,
            base_revision: self.authority.revision(),
            actor: self.principals[actor].clone(),
            action,
        }
    }

    fn admit(&mut self, actor: usize, action: Action) -> Change {
        let change = self.change(actor, action);
        let revision = self.authority.revision();
        assert_eq!(
            self.authority.apply(change.clone()).unwrap(),
            Outcome {
                admission: Admission::NewlyAdmitted,
                revision: revision + 1,
            }
        );
        change
    }

    fn allows(&self, principal: usize, project: usize, permission: ProjectPermission) -> bool {
        self.authority.allows(
            &self.organization,
            &self.principals[principal],
            &self.projects[project],
            permission,
        )
    }
}

fn capabilities(principal: &PrincipalId, values: &[OrganizationCapability]) -> Action {
    Action::SetOrganizationCapabilities {
        principal: principal.clone(),
        capabilities: values.iter().copied().collect(),
    }
}

fn permissions(
    principal: &PrincipalId,
    project: &ProjectId,
    values: &[ProjectPermission],
) -> Action {
    Action::SetProjectPermissions {
        principal: principal.clone(),
        project: project.clone(),
        permissions: values.iter().copied().collect(),
    }
}

fn refused_without_changes(authority: &mut Authority, change: Change) {
    let before = authority.clone();
    assert_eq!(authority.apply(change), Err(Refused));
    assert_eq!(*authority, before);
}

#[test]
fn identifiers_require_canonical_lowercase_ulids() {
    macro_rules! check {
        ($kind:ident) => {
            assert!($kind::parse(&format!("7{}", "z".repeat(25))).is_ok());
            for invalid in [
                String::new(),
                "0".repeat(25),
                "0".repeat(27),
                format!("8{}", "0".repeat(25)),
                format!("0{}A", "0".repeat(24)),
                format!("0{}é", "0".repeat(23)),
            ]
            .into_iter()
            .chain(
                ['i', 'l', 'o', 'u', '-', ' '].map(|value| format!("0{}{value}", "0".repeat(24))),
            ) {
                assert!($kind::parse(&invalid).is_err());
            }
        };
    }
    check!(OrganizationId);
    check!(PrincipalId);
    check!(ProjectId);
    check!(OperationId);
}

#[test]
fn administrative_capabilities_and_membership_grant_no_content() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.authority.principals[&fixture.principals[0]].capabilities,
        CAPABILITIES.into_iter().collect()
    );
    for principal in 0..4 {
        if principal != 0 {
            assert!(fixture.authority.principals[&fixture.principals[principal]]
                .capabilities
                .is_empty());
        }
        for project in 0..2 {
            for permission in PERMISSIONS {
                assert!(!fixture.allows(principal, project, permission));
            }
        }
    }
}

#[test]
fn project_permissions_are_exact_independent_and_replaceable() {
    let mut fixture = Fixture::new();
    for granted in PERMISSIONS {
        fixture.admit(
            0,
            permissions(&fixture.principals[1], &fixture.projects[0], &[granted]),
        );
        for queried in PERMISSIONS {
            assert_eq!(fixture.allows(1, 0, queried), queried == granted);
            assert!(!fixture.allows(1, 1, queried));
            assert!(!fixture.allows(0, 0, queried));
            assert!(!fixture.allows(2, 0, queried));
        }
    }
    fixture.admit(
        0,
        permissions(&fixture.principals[1], &fixture.projects[0], &[]),
    );
    for permission in PERMISSIONS {
        assert!(!fixture.allows(1, 0, permission));
    }
}

#[test]
fn each_change_requires_its_organization_capability() {
    let mut fixture = Fixture::new();
    let newcomer = PrincipalId::parse(&synthetic(14)).unwrap();
    let project = ProjectId::parse(&synthetic(22)).unwrap();
    let actions = [
        Action::AddPrincipal {
            principal: newcomer.clone(),
        },
        Action::CreateProject {
            project: project.clone(),
        },
        capabilities(&fixture.principals[2], &[]),
        permissions(&fixture.principals[2], &fixture.projects[0], &PERMISSIONS),
        Action::RevokePrincipal {
            principal: fixture.principals[2].clone(),
        },
    ];
    for action in actions {
        let change = fixture.change(1, action);
        refused_without_changes(&mut fixture.authority, change);
    }
    fixture.admit(
        0,
        capabilities(
            &fixture.principals[1],
            &[OrganizationCapability::ManageMembers],
        ),
    );
    fixture.admit(
        1,
        Action::AddPrincipal {
            principal: newcomer,
        },
    );
    let change = fixture.change(1, capabilities(&fixture.principals[1], &CAPABILITIES));
    refused_without_changes(&mut fixture.authority, change);
    let change = fixture.change(
        1,
        Action::CreateProject {
            project: project.clone(),
        },
    );
    refused_without_changes(&mut fixture.authority, change);
    fixture.admit(
        0,
        capabilities(
            &fixture.principals[1],
            &[OrganizationCapability::ManageProjects],
        ),
    );
    fixture.admit(1, Action::CreateProject { project });
    fixture.admit(
        0,
        capabilities(
            &fixture.principals[1],
            &[OrganizationCapability::GrantProjectAccess],
        ),
    );
    assert!(!fixture.allows(1, 0, ProjectPermission::ReadContent));
    fixture.admit(
        1,
        permissions(
            &fixture.principals[2],
            &fixture.projects[0],
            &[ProjectPermission::ReadContent],
        ),
    );
    assert!(fixture.allows(2, 0, ProjectPermission::ReadContent));
}

#[test]
fn organization_assignment_requires_capabilities_the_actor_has() {
    let mut fixture = Fixture::new();
    let management = [OrganizationCapability::ManageOrganizationCapabilities];
    fixture.admit(0, capabilities(&fixture.principals[1], &management));
    for target in [1, 2] {
        let change = fixture.change(1, capabilities(&fixture.principals[target], &CAPABILITIES));
        refused_without_changes(&mut fixture.authority, change);
    }
    fixture.admit(1, capabilities(&fixture.principals[2], &management));
    fixture.admit(1, capabilities(&fixture.principals[2], &[]));
    assert!(fixture.authority.principals[&fixture.principals[2]]
        .capabilities
        .is_empty());
}

#[test]
fn absent_revoked_and_foreign_identities_fail_closed() {
    let mut fixture = Fixture::new();
    let absent = PrincipalId::parse(&synthetic(99)).unwrap();
    let unknown_project = ProjectId::parse(&synthetic(99)).unwrap();
    fixture.admit(
        0,
        permissions(&fixture.principals[1], &fixture.projects[0], &PERMISSIONS),
    );
    assert!(!fixture.authority.allows(
        &fixture.other_organization,
        &fixture.principals[1],
        &fixture.projects[0],
        ProjectPermission::ReadContent
    ));
    assert!(!fixture.authority.allows(
        &fixture.organization,
        &absent,
        &fixture.projects[0],
        ProjectPermission::ReadContent
    ));
    assert!(!fixture.authority.allows(
        &fixture.organization,
        &fixture.principals[1],
        &unknown_project,
        ProjectPermission::ReadContent
    ));
    let mut change = fixture.change(0, capabilities(&fixture.principals[2], &[]));
    change.actor = absent.clone();
    refused_without_changes(&mut fixture.authority, change);
    let mut change = fixture.change(0, capabilities(&fixture.principals[2], &[]));
    change.organization = fixture.other_organization.clone();
    refused_without_changes(&mut fixture.authority, change);
    fixture.admit(
        0,
        Action::RevokePrincipal {
            principal: fixture.principals[1].clone(),
        },
    );
    assert!(!fixture.allows(1, 0, ProjectPermission::ReadContent));
    for target in [absent, fixture.principals[1].clone()] {
        for action in [
            capabilities(&target, &[]),
            permissions(&target, &fixture.projects[0], &PERMISSIONS),
            Action::RevokePrincipal {
                principal: target.clone(),
            },
        ] {
            let change = fixture.change(0, action);
            refused_without_changes(&mut fixture.authority, change);
        }
    }
    let change = fixture.change(
        0,
        permissions(&fixture.principals[2], &unknown_project, &PERMISSIONS),
    );
    refused_without_changes(&mut fixture.authority, change);
    let change = fixture.change(
        1,
        Action::CreateProject {
            project: unknown_project,
        },
    );
    refused_without_changes(&mut fixture.authority, change);
}

#[test]
fn compare_and_set_admits_one_ordered_change_and_denial_reserves_nothing() {
    let mut fixture = Fixture::new();
    let first = fixture.change(
        0,
        permissions(
            &fixture.principals[1],
            &fixture.projects[0],
            &[ProjectPermission::ReadContent],
        ),
    );
    let mut second = fixture.change(
        0,
        permissions(
            &fixture.principals[2],
            &fixture.projects[1],
            &[ProjectPermission::WriteWork],
        ),
    );
    assert_eq!(first.base_revision, second.base_revision);
    fixture.authority.apply(first).unwrap();
    refused_without_changes(&mut fixture.authority, second.clone());
    assert!(!fixture.allows(2, 1, ProjectPermission::WriteWork));
    second.base_revision = fixture.authority.revision();
    fixture.authority.apply(second).unwrap();
    assert!(fixture.allows(1, 0, ProjectPermission::ReadContent));
    assert!(fixture.allows(2, 1, ProjectPermission::WriteWork));
}

#[test]
fn operation_replay_requires_every_original_argument_and_returns_original_revision() {
    let mut fixture = Fixture::new();
    let original = fixture.admit(
        0,
        permissions(
            &fixture.principals[1],
            &fixture.projects[0],
            &[ProjectPermission::ReadContent],
        ),
    );
    fixture.admit(0, capabilities(&fixture.principals[2], &[]));
    let before = fixture.authority.clone();
    assert_eq!(
        fixture.authority.apply(original.clone()).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: original.base_revision + 1,
        }
    );
    assert_eq!(fixture.authority, before);
    let mut changes = vec![original.clone(); 6];
    changes[0].organization = fixture.other_organization.clone();
    changes[1].actor = fixture.principals[1].clone();
    changes[2].base_revision += 1;
    changes[3].action = permissions(
        &fixture.principals[1],
        &fixture.projects[1],
        &[ProjectPermission::ReadContent],
    );
    changes[4].action = permissions(
        &fixture.principals[2],
        &fixture.projects[0],
        &[ProjectPermission::ReadContent],
    );
    changes[5].action = capabilities(&fixture.principals[1], &[]);
    for change in changes {
        refused_without_changes(&mut fixture.authority, change);
    }
    let mut changed_set = original;
    changed_set.action = permissions(&fixture.principals[1], &fixture.projects[0], &PERMISSIONS);
    refused_without_changes(&mut fixture.authority, changed_set);
}

#[test]
fn historical_replay_does_not_restore_access_or_authorize_a_new_change() {
    let mut fixture = Fixture::new();
    let grant = fixture.admit(
        0,
        permissions(
            &fixture.principals[1],
            &fixture.projects[0],
            &[ProjectPermission::ReadContent],
        ),
    );
    fixture.admit(
        0,
        permissions(&fixture.principals[1], &fixture.projects[0], &[]),
    );
    fixture.admit(
        0,
        capabilities(
            &fixture.principals[0],
            &[
                OrganizationCapability::ManageMembers,
                OrganizationCapability::ManageProjects,
                OrganizationCapability::ManageOrganizationCapabilities,
            ],
        ),
    );
    let before = fixture.authority.clone();
    assert_eq!(
        fixture.authority.apply(grant.clone()).unwrap().admission,
        Admission::Historical
    );
    assert_eq!(fixture.authority, before);
    assert!(!fixture.allows(1, 0, ProjectPermission::ReadContent));
    let mut new_grant = fixture.change(0, grant.action);
    refused_without_changes(&mut fixture.authority, new_grant.clone());
    new_grant.actor = fixture.principals[1].clone();
    refused_without_changes(&mut fixture.authority, new_grant);
}

#[test]
fn revocation_clears_all_capabilities_and_grants_and_preserves_identity_history() {
    let mut fixture = Fixture::new();
    let mut historical =
        vec![fixture.admit(0, capabilities(&fixture.principals[1], &CAPABILITIES))];
    for project in fixture.projects.clone() {
        historical.push(fixture.admit(
            0,
            permissions(&fixture.principals[1], &project, &PERMISSIONS),
        ));
    }
    let prior = fixture.admit(
        1,
        Action::CreateProject {
            project: ProjectId::parse(&synthetic(22)).unwrap(),
        },
    );
    fixture.admit(
        0,
        Action::RevokePrincipal {
            principal: fixture.principals[1].clone(),
        },
    );
    let member = &fixture.authority.principals[&fixture.principals[1]];
    assert!(!member.active);
    assert!(member.capabilities.is_empty());
    assert!(member.grants.is_empty());
    let before = fixture.authority.clone();
    for change in historical {
        assert_eq!(
            fixture.authority.apply(change).unwrap().admission,
            Admission::Historical
        );
        assert_eq!(fixture.authority, before);
    }
    refused_without_changes(&mut fixture.authority, prior);
    let change = fixture.change(
        0,
        Action::AddPrincipal {
            principal: fixture.principals[1].clone(),
        },
    );
    refused_without_changes(&mut fixture.authority, change);
    let change = fixture.change(
        0,
        Action::CreateProject {
            project: fixture.projects[0].clone(),
        },
    );
    refused_without_changes(&mut fixture.authority, change);
    for project in 0..2 {
        for permission in PERMISSIONS {
            assert!(!fixture.allows(1, project, permission));
        }
    }
}

#[test]
fn revoking_the_last_manager_leaves_no_administrative_escape() {
    let mut fixture = Fixture::new();
    let revoked = fixture.admit(
        0,
        Action::RevokePrincipal {
            principal: fixture.principals[0].clone(),
        },
    );
    refused_without_changes(&mut fixture.authority, revoked);
    for actor in 0..4 {
        let change = fixture.change(
            actor,
            capabilities(&fixture.principals[actor], &CAPABILITIES),
        );
        refused_without_changes(&mut fixture.authority, change);
    }
}

#[test]
fn principal_quota_includes_revoked_members_without_eviction() {
    let mut fixture = Fixture::new();
    for value in 1000..1124 {
        fixture.admit(
            0,
            Action::AddPrincipal {
                principal: PrincipalId::parse(&synthetic(value)).unwrap(),
            },
        );
    }
    assert_eq!(fixture.authority.principals.len(), 128);
    fixture.admit(
        0,
        Action::RevokePrincipal {
            principal: fixture.principals[1].clone(),
        },
    );
    let change = fixture.change(
        0,
        Action::AddPrincipal {
            principal: PrincipalId::parse(&synthetic(2000)).unwrap(),
        },
    );
    refused_without_changes(&mut fixture.authority, change);
    assert_eq!(fixture.authority.principals.len(), 128);
}

#[test]
fn project_quota_and_duplicate_ids_do_not_evict_or_advance_revision() {
    let mut fixture = Fixture::new();
    let duplicate = fixture.change(
        0,
        Action::AddPrincipal {
            principal: fixture.principals[2].clone(),
        },
    );
    refused_without_changes(&mut fixture.authority, duplicate);
    for value in 1000..1062 {
        fixture.admit(
            0,
            Action::CreateProject {
                project: ProjectId::parse(&synthetic(value)).unwrap(),
            },
        );
    }
    assert_eq!(fixture.authority.projects.len(), 64);
    for project in [
        fixture.projects[0].clone(),
        ProjectId::parse(&synthetic(2000)).unwrap(),
    ] {
        let change = fixture.change(0, Action::CreateProject { project });
        refused_without_changes(&mut fixture.authority, change);
    }
}

#[test]
fn operation_quota_preserves_receipts_and_historical_replay() {
    let mut fixture = Fixture::new();
    let original = fixture.admit(0, capabilities(&fixture.principals[2], &[]));
    while fixture.authority.receipts.len() < 1024 {
        fixture.admit(
            0,
            permissions(&fixture.principals[1], &fixture.projects[0], &[]),
        );
    }
    assert_eq!(fixture.authority.receipts.len(), 1024);
    let change = fixture.change(0, capabilities(&fixture.principals[2], &[]));
    refused_without_changes(&mut fixture.authority, change);
    let before = fixture.authority.clone();
    assert_eq!(
        fixture.authority.apply(original.clone()).unwrap(),
        Outcome {
            admission: Admission::Historical,
            revision: original.base_revision + 1,
        }
    );
    assert_eq!(fixture.authority, before);
}

#[test]
fn revision_overflow_refuses_before_any_mutation() {
    let mut fixture = Fixture::new();
    let historical = fixture.admit(0, capabilities(&fixture.principals[2], &[]));
    fixture.authority.revision = u64::MAX;
    let change = fixture.change(
        0,
        Action::RevokePrincipal {
            principal: fixture.principals[2].clone(),
        },
    );
    refused_without_changes(&mut fixture.authority, change);
    let before = fixture.authority.clone();
    assert_eq!(
        fixture.authority.apply(historical).unwrap().admission,
        Admission::Historical
    );
    assert_eq!(fixture.authority, before);
}
