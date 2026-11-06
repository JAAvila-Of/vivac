use std::collections::BTreeMap;

use ed25519_dalek::{Signature, VerifyingKey};

use super::{
    Action, Admission, Authority, Change, CredentialId, DeviceId, OperationId,
    OrganizationCapability, OrganizationId, Outcome, PrincipalId, ProjectId, ProjectPermission,
    Refused, Result, MAX_OPERATIONS,
};

#[cfg(test)]
mod tests;

const DOMAIN: &[u8] = b"vivac.team.change.v1\0";
pub(crate) const MAX_ENVELOPE_BYTES: usize = DOMAIN.len() + 5 * 26 + 8 + 1 + 2 * 26 + 1;

#[derive(Clone, Debug)]
pub(crate) struct KnownDevice {
    organization: OrganizationId,
    principal: PrincipalId,
    device: DeviceId,
    credential: CredentialId,
    key: VerifyingKey,
}

impl KnownDevice {
    pub(crate) fn new(
        organization: OrganizationId,
        principal: PrincipalId,
        device: DeviceId,
        credential: CredentialId,
        key: [u8; 32],
    ) -> Result<Self> {
        let key = VerifyingKey::from_bytes(&key).map_err(|_| Refused)?;
        if key.is_weak() {
            return Err(Refused);
        }
        Ok(Self {
            organization,
            principal,
            device,
            credential,
            key,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceRequest {
    pub change: Change,
    pub device: DeviceId,
    pub credential: CredentialId,
}

impl DeviceRequest {
    pub(crate) fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(MAX_ENVELOPE_BYTES);
        bytes.extend_from_slice(DOMAIN);
        for value in [
            &self.change.organization.0,
            &self.change.actor.0,
            &self.device.0,
            &self.credential.0,
            &self.change.operation.0,
        ] {
            bytes.extend_from_slice(value.as_bytes());
        }
        bytes.extend_from_slice(&self.change.base_revision.to_be_bytes());
        match &self.change.action {
            Action::AddPrincipal { principal } => {
                bytes.push(1);
                bytes.extend_from_slice(principal.0.as_bytes());
            }
            Action::SetOrganizationCapabilities {
                principal,
                capabilities,
            } => {
                bytes.push(2);
                bytes.extend_from_slice(principal.0.as_bytes());
                bytes.push(capabilities.iter().fold(0, |mask, capability| {
                    mask | match capability {
                        OrganizationCapability::ManageMembers => 1,
                        OrganizationCapability::ManageProjects => 2,
                        OrganizationCapability::GrantProjectAccess => 4,
                        OrganizationCapability::ManageOrganizationCapabilities => 8,
                    }
                }));
            }
            Action::CreateProject { project } => {
                bytes.push(3);
                bytes.extend_from_slice(project.0.as_bytes());
            }
            Action::SetProjectPermissions {
                principal,
                project,
                permissions,
            } => {
                bytes.push(4);
                bytes.extend_from_slice(principal.0.as_bytes());
                bytes.extend_from_slice(project.0.as_bytes());
                bytes.push(permissions.iter().fold(0, |mask, permission| {
                    mask | match permission {
                        ProjectPermission::ReadContent => 1,
                        ProjectPermission::WriteWork => 2,
                        ProjectPermission::ProposeNorm => 4,
                        ProjectPermission::ReadReports => 8,
                        ProjectPermission::ReadAudit => 16,
                    }
                }));
            }
            Action::RevokePrincipal { principal } => {
                bytes.push(5);
                bytes.extend_from_slice(principal.0.as_bytes());
            }
        }
        bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignedRequest {
    pub request: DeviceRequest,
    pub signature: [u8; 64],
}

impl SignedRequest {
    pub(crate) fn verify(&self, device: &KnownDevice) -> Result<Change> {
        if self.request.change.organization != device.organization
            || self.request.change.actor != device.principal
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
            .map_err(|_| Refused)?;
        Ok(self.request.change.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SignedReceipt {
    request: DeviceRequest,
    key: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignedAuthority {
    authority: Authority,
    receipts: BTreeMap<OperationId, SignedReceipt>,
}

impl SignedAuthority {
    pub(crate) fn bootstrap(organization: OrganizationId, initial: PrincipalId) -> Self {
        Self {
            authority: Authority::bootstrap(organization, initial),
            receipts: BTreeMap::new(),
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.authority.revision()
    }

    pub(crate) fn allows(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        project: &ProjectId,
        permission: ProjectPermission,
    ) -> bool {
        self.authority
            .allows(organization, principal, project, permission)
    }

    pub(crate) fn apply_signed(
        &mut self,
        request: &SignedRequest,
        device: &KnownDevice,
    ) -> Result<Outcome> {
        let change = request.verify(device)?;
        if change.organization != self.authority.organization {
            return Err(Refused);
        }
        self.authority.active(&change.actor)?;
        let evidence = SignedReceipt {
            request: request.request.clone(),
            key: device.key.to_bytes(),
        };
        let historical = self.receipts.get(&change.operation);
        if historical.is_some_and(|receipt| receipt != &evidence)
            || (historical.is_none() && self.receipts.len() >= MAX_OPERATIONS)
        {
            return Err(Refused);
        }
        let outcome = self.authority.apply(change.clone())?;
        match outcome.admission {
            Admission::NewlyAdmitted => {
                self.receipts.insert(change.operation, evidence);
            }
            Admission::Historical if historical.is_none() => return Err(Refused),
            Admission::Historical => {}
        }
        Ok(outcome)
    }
}
