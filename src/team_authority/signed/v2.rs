use std::collections::BTreeSet;

use super::*;

pub(crate) mod registry;

#[cfg(test)]
mod tests;

const DOMAIN: &[u8] = b"vivac.team.change.v2\0";
pub(crate) const MAX_ENVELOPE_BYTES: usize = 213;
const TAG_OFFSET: usize = DOMAIN.len() + 5 * 26 + 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum V2Action {
    Classic(Action),
    SetOwnAdditionalDevice {
        principal: PrincipalId,
        enabled: bool,
    },
    SetEnrollmentPolicyAdministrator {
        principal: PrincipalId,
        enabled: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceRequestV2 {
    pub(crate) organization: OrganizationId,
    pub(crate) actor: PrincipalId,
    pub(crate) device: DeviceId,
    pub(crate) credential: CredentialId,
    pub(crate) operation: OperationId,
    pub(crate) base_revision: u64,
    pub(crate) action: V2Action,
}

impl DeviceRequestV2 {
    fn classic_request(&self, action: Action) -> DeviceRequest {
        DeviceRequest {
            change: Change {
                organization: self.organization.clone(),
                actor: self.actor.clone(),
                operation: self.operation.clone(),
                base_revision: self.base_revision,
                action,
            },
            device: self.device.clone(),
            credential: self.credential.clone(),
        }
    }

    pub(crate) fn parse_canonical(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_ENVELOPE_BYTES || !bytes.starts_with(DOMAIN) {
            return Err(Refused);
        }
        let tag = *bytes.get(TAG_OFFSET).ok_or(Refused)?;
        if !(1..=7).contains(&tag) {
            return Err(Refused);
        }
        let mut classic = super::DOMAIN.to_vec();
        classic.extend_from_slice(&bytes[DOMAIN.len()..]);
        let enabled = if tag >= 6 {
            if bytes.len() != TAG_OFFSET + 28 {
                return Err(Refused);
            }
            let enabled = match classic.pop().ok_or(Refused)? {
                0 => false,
                1 => true,
                _ => return Err(Refused),
            };
            classic[TAG_OFFSET] = 1;
            Some(enabled)
        } else {
            None
        };
        let parsed = DeviceRequest::parse_canonical(&classic)?;
        let action = match (tag, enabled, parsed.change.action) {
            (6, Some(enabled), Action::AddPrincipal { principal }) => {
                V2Action::SetOwnAdditionalDevice { principal, enabled }
            }
            (7, Some(enabled), Action::AddPrincipal { principal }) => {
                V2Action::SetEnrollmentPolicyAdministrator { principal, enabled }
            }
            (1..=5, None, action) => V2Action::Classic(action),
            _ => return Err(Refused),
        };
        let request = Self {
            organization: parsed.change.organization,
            actor: parsed.change.actor,
            operation: parsed.change.operation,
            base_revision: parsed.change.base_revision,
            device: parsed.device,
            credential: parsed.credential,
            action,
        };
        if request.canonical_bytes() != bytes {
            return Err(Refused);
        }
        Ok(request)
    }

    pub(crate) fn canonical_bytes(&self) -> Vec<u8> {
        if let V2Action::Classic(action) = &self.action {
            let classic = self.classic_request(action.clone()).canonical_bytes();
            let mut bytes = DOMAIN.to_vec();
            bytes.extend_from_slice(&classic[super::DOMAIN.len()..]);
            return bytes;
        }
        let mut bytes = DOMAIN.to_vec();
        for value in [
            self.organization.as_str(),
            self.actor.as_str(),
            self.device.as_str(),
            self.credential.as_str(),
            self.operation.as_str(),
        ] {
            bytes.extend_from_slice(value.as_bytes());
        }
        bytes.extend_from_slice(&self.base_revision.to_be_bytes());
        match &self.action {
            V2Action::SetOwnAdditionalDevice { principal, enabled }
            | V2Action::SetEnrollmentPolicyAdministrator { principal, enabled } => {
                bytes.push(
                    if matches!(self.action, V2Action::SetOwnAdditionalDevice { .. }) {
                        6
                    } else {
                        7
                    },
                );
                bytes.extend_from_slice(principal.as_str().as_bytes());
                bytes.push(u8::from(*enabled));
            }
            V2Action::Classic(_) => unreachable!(),
        }
        bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignedRequestV2 {
    pub(crate) request: DeviceRequestV2,
    pub(crate) signature: [u8; 64],
}

impl SignedRequestV2 {
    pub(crate) fn verify(&self, device: &KnownDevice) -> Result<()> {
        if self.request.organization != device.organization
            || self.request.actor != device.principal
            || self.request.device != device.device
            || self.request.credential != device.credential
        {
            return Err(Refused);
        }
        device
            .key
            .verify_strict(
                &self.request.canonical_bytes(),
                &Signature::from_bytes(&self.signature),
            )
            .map_err(|_| Refused)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SignedReceiptV2 {
    request: DeviceRequestV2,
    key: [u8; 32],
    revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignedAuthorityV2 {
    core: Authority,
    policy_admins: BTreeSet<PrincipalId>,
    own_grants: BTreeSet<PrincipalId>,
    receipts: BTreeMap<OperationId, SignedReceiptV2>,
}

impl SignedAuthorityV2 {
    pub(crate) fn bootstrap(
        organization: OrganizationId,
        initial: PrincipalId,
        policy_admin: PrincipalId,
    ) -> Result<Self> {
        if policy_admin != initial {
            return Err(Refused);
        }
        Ok(Self {
            core: Authority::bootstrap(
                organization,
                initial,
                [
                    OrganizationCapability::ManageMembers,
                    OrganizationCapability::ManageProjects,
                    OrganizationCapability::GrantProjectAccess,
                    OrganizationCapability::ManageOrganizationCapabilities,
                ]
                .into_iter()
                .collect(),
            ),
            policy_admins: [policy_admin].into_iter().collect(),
            own_grants: BTreeSet::new(),
            receipts: BTreeMap::new(),
        })
    }

    pub(crate) fn can_add_own_device(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> bool {
        organization == &self.core.organization
            && self.core.active(principal).is_ok()
            && self.own_grants.contains(principal)
    }

    pub(crate) fn apply_signed(
        &mut self,
        signed: &SignedRequestV2,
        device: &KnownDevice,
    ) -> Result<Outcome> {
        signed.verify(device)?;
        let request = &signed.request;
        if request.organization != self.core.organization {
            return Err(Refused);
        }
        let actor = self.core.active(&request.actor)?;
        if let Some(receipt) = self.receipts.get(&request.operation) {
            return if receipt.request == *request && receipt.key == device.key.to_bytes() {
                Ok(Outcome {
                    admission: Admission::Historical,
                    revision: receipt.revision,
                })
            } else {
                Err(Refused)
            };
        }
        if request.base_revision != self.core.revision || self.receipts.len() >= MAX_OPERATIONS {
            return Err(Refused);
        }
        let revision = self.core.revision.checked_add(1).ok_or(Refused)?;
        match &request.action {
            V2Action::Classic(action) => {
                self.core
                    .apply(request.classic_request(action.clone()).change)?;
                if let Action::RevokePrincipal { principal } = action {
                    self.policy_admins.remove(principal);
                    self.own_grants.remove(principal);
                }
            }
            V2Action::SetOwnAdditionalDevice { principal, enabled }
            | V2Action::SetEnrollmentPolicyAdministrator { principal, enabled } => {
                self.core.active(principal)?;
                if !self.policy_admins.contains(&request.actor)
                    || (matches!(
                        request.action,
                        V2Action::SetEnrollmentPolicyAdministrator { .. }
                    ) && !actor
                        .capabilities
                        .contains(&OrganizationCapability::ManageOrganizationCapabilities))
                {
                    return Err(Refused);
                }
                let grants = if matches!(request.action, V2Action::SetOwnAdditionalDevice { .. }) {
                    &mut self.own_grants
                } else {
                    &mut self.policy_admins
                };
                if *enabled {
                    grants.insert(principal.clone());
                } else {
                    grants.remove(principal);
                }
                self.core.revision = revision;
            }
        }
        self.receipts.insert(
            request.operation.clone(),
            SignedReceiptV2 {
                request: request.clone(),
                key: device.key.to_bytes(),
                revision,
            },
        );
        Ok(Outcome {
            admission: Admission::NewlyAdmitted,
            revision,
        })
    }

    pub(crate) fn revision(&self) -> u64 {
        self.core.revision()
    }

    pub(crate) fn can_manage_enrollment_policy(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> bool {
        organization == &self.core.organization
            && self.core.active(principal).is_ok()
            && self.policy_admins.contains(principal)
    }

    pub(crate) fn allows(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        project: &ProjectId,
        permission: ProjectPermission,
    ) -> bool {
        self.core
            .allows(organization, principal, project, permission)
    }
}
