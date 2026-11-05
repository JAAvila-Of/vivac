use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Refused;

type Result<T> = std::result::Result<T, Refused>;

const MAX_PRINCIPALS: usize = 128;
const MAX_PROJECTS: usize = 64;
const MAX_OPERATIONS: usize = 1024;

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
        pub(crate) struct $name(String);

        impl $name {
            pub(crate) fn parse(value: &str) -> Result<Self> {
                let bytes = value.as_bytes();
                if bytes.len() != 26
                    || !(b'0'..=b'7').contains(&bytes[0])
                    || !bytes
                        .iter()
                        .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(byte))
                {
                    return Err(Refused);
                }
                Ok(Self(value.to_owned()))
            }
        }
    };
}

identifier!(OrganizationId);
identifier!(PrincipalId);
identifier!(ProjectId);
identifier!(OperationId);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum OrganizationCapability {
    ManageMembers,
    ManageProjects,
    GrantProjectAccess,
    ManageOrganizationCapabilities,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ProjectPermission {
    ReadContent,
    WriteWork,
    ProposeNorm,
    ReadReports,
    ReadAudit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Action {
    AddPrincipal {
        principal: PrincipalId,
    },
    SetOrganizationCapabilities {
        principal: PrincipalId,
        capabilities: BTreeSet<OrganizationCapability>,
    },
    CreateProject {
        project: ProjectId,
    },
    SetProjectPermissions {
        principal: PrincipalId,
        project: ProjectId,
        permissions: BTreeSet<ProjectPermission>,
    },
    RevokePrincipal {
        principal: PrincipalId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Change {
    pub organization: OrganizationId,
    pub operation: OperationId,
    pub base_revision: u64,
    pub actor: PrincipalId,
    pub action: Action,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Admission {
    NewlyAdmitted,
    Historical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Outcome {
    pub admission: Admission,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Principal {
    active: bool,
    capabilities: BTreeSet<OrganizationCapability>,
    grants: BTreeMap<ProjectId, BTreeSet<ProjectPermission>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Receipt {
    change: Change,
    revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Authority {
    organization: OrganizationId,
    revision: u64,
    principals: BTreeMap<PrincipalId, Principal>,
    projects: BTreeSet<ProjectId>,
    receipts: BTreeMap<OperationId, Receipt>,
}

impl Authority {
    pub(crate) fn bootstrap(organization: OrganizationId, initial: PrincipalId) -> Self {
        let principal = Principal {
            active: true,
            capabilities: [
                OrganizationCapability::ManageMembers,
                OrganizationCapability::ManageProjects,
                OrganizationCapability::GrantProjectAccess,
                OrganizationCapability::ManageOrganizationCapabilities,
            ]
            .into_iter()
            .collect(),
            grants: BTreeMap::new(),
        };
        Self {
            organization,
            revision: 0,
            principals: [(initial, principal)].into_iter().collect(),
            projects: BTreeSet::new(),
            receipts: BTreeMap::new(),
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn allows(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        project: &ProjectId,
        permission: ProjectPermission,
    ) -> bool {
        organization == &self.organization
            && self.projects.contains(project)
            && self.principals.get(principal).is_some_and(|member| {
                member.active
                    && member
                        .grants
                        .get(project)
                        .is_some_and(|grants| grants.contains(&permission))
            })
    }

    pub(crate) fn apply(&mut self, change: Change) -> Result<Outcome> {
        if change.organization != self.organization {
            return Err(Refused);
        }
        let actor = self
            .principals
            .get(&change.actor)
            .filter(|member| member.active)
            .ok_or(Refused)?;
        if let Some(receipt) = self.receipts.get(&change.operation) {
            return if receipt.change == change {
                Ok(Outcome {
                    admission: Admission::Historical,
                    revision: receipt.revision,
                })
            } else {
                Err(Refused)
            };
        }
        if change.base_revision != self.revision || self.receipts.len() >= MAX_OPERATIONS {
            return Err(Refused);
        }
        let revision = self.revision.checked_add(1).ok_or(Refused)?;
        let required = match &change.action {
            Action::AddPrincipal { principal } => {
                if self.principals.contains_key(principal)
                    || self.principals.len() >= MAX_PRINCIPALS
                {
                    return Err(Refused);
                }
                OrganizationCapability::ManageMembers
            }
            Action::SetOrganizationCapabilities {
                principal,
                capabilities,
            } => {
                self.active(principal)?;
                if !capabilities.is_subset(&actor.capabilities) {
                    return Err(Refused);
                }
                OrganizationCapability::ManageOrganizationCapabilities
            }
            Action::CreateProject { project } => {
                if self.projects.contains(project) || self.projects.len() >= MAX_PROJECTS {
                    return Err(Refused);
                }
                OrganizationCapability::ManageProjects
            }
            Action::SetProjectPermissions {
                principal, project, ..
            } => {
                self.active(principal)?;
                if !self.projects.contains(project) {
                    return Err(Refused);
                }
                OrganizationCapability::GrantProjectAccess
            }
            Action::RevokePrincipal { principal } => {
                self.active(principal)?;
                OrganizationCapability::ManageMembers
            }
        };
        if !actor.capabilities.contains(&required) {
            return Err(Refused);
        }
        match &change.action {
            Action::AddPrincipal { principal } => {
                self.principals.insert(
                    principal.clone(),
                    Principal {
                        active: true,
                        capabilities: BTreeSet::new(),
                        grants: BTreeMap::new(),
                    },
                );
            }
            Action::SetOrganizationCapabilities {
                principal,
                capabilities,
            } => {
                self.principals
                    .get_mut(principal)
                    .ok_or(Refused)?
                    .capabilities = capabilities.clone();
            }
            Action::CreateProject { project } => {
                self.projects.insert(project.clone());
            }
            Action::SetProjectPermissions {
                principal,
                project,
                permissions,
            } => {
                let grants = &mut self.principals.get_mut(principal).ok_or(Refused)?.grants;
                if permissions.is_empty() {
                    grants.remove(project);
                } else {
                    grants.insert(project.clone(), permissions.clone());
                }
            }
            Action::RevokePrincipal { principal } => {
                let member = self.principals.get_mut(principal).ok_or(Refused)?;
                member.active = false;
                member.capabilities.clear();
                member.grants.clear();
            }
        }
        self.receipts
            .insert(change.operation.clone(), Receipt { change, revision });
        self.revision = revision;
        Ok(Outcome {
            admission: Admission::NewlyAdmitted,
            revision,
        })
    }

    fn active(&self, principal: &PrincipalId) -> Result<&Principal> {
        self.principals
            .get(principal)
            .filter(|member| member.active)
            .ok_or(Refused)
    }
}
