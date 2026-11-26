use super::*;

#[cfg(feature = "coordinator-probe")]
pub(crate) mod admission;

#[cfg(test)]
mod tests;

const DOMAIN: &[u8] = b"vivac.team.credential-revocation.v1\0";
pub(crate) const REVOCATION_BYTES: usize = DOMAIN.len() + 7 * 26 + 8;
const MAX_BINDINGS: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RevocationRequest {
    pub(crate) organization: OrganizationId,
    pub(crate) actor: PrincipalId,
    pub(crate) device: DeviceId,
    pub(crate) credential: CredentialId,
    pub(crate) operation: OperationId,
    pub(crate) base_revision: u64,
    pub(crate) target_device: DeviceId,
    pub(crate) target_credential: CredentialId,
}

impl RevocationRequest {
    pub(crate) fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(REVOCATION_BYTES);
        bytes.extend_from_slice(DOMAIN);
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
        bytes.extend_from_slice(self.target_device.as_str().as_bytes());
        bytes.extend_from_slice(self.target_credential.as_str().as_bytes());
        bytes
    }

    pub(crate) fn parse_canonical(bytes: &[u8]) -> Result<Self> {
        fn identifier<'a>(remaining: &mut &'a [u8]) -> Result<&'a str> {
            let bytes = remaining.get(..26).ok_or(Refused)?;
            *remaining = &remaining[26..];
            std::str::from_utf8(bytes).map_err(|_| Refused)
        }

        if bytes.len() != REVOCATION_BYTES {
            return Err(Refused);
        }
        let mut remaining = bytes.strip_prefix(DOMAIN).ok_or(Refused)?;
        let organization = OrganizationId::parse(identifier(&mut remaining)?)?;
        let actor = PrincipalId::parse(identifier(&mut remaining)?)?;
        let device = DeviceId::parse(identifier(&mut remaining)?)?;
        let credential = CredentialId::parse(identifier(&mut remaining)?)?;
        let operation = OperationId::parse(identifier(&mut remaining)?)?;
        let revision = remaining.get(..8).ok_or(Refused)?;
        let base_revision = u64::from_be_bytes(revision.try_into().map_err(|_| Refused)?);
        remaining = &remaining[8..];
        let target_device = DeviceId::parse(identifier(&mut remaining)?)?;
        let target_credential = CredentialId::parse(identifier(&mut remaining)?)?;
        let request = Self {
            organization,
            actor,
            device,
            credential,
            operation,
            base_revision,
            target_device,
            target_credential,
        };
        if !remaining.is_empty() || request.canonical_bytes() != bytes {
            return Err(Refused);
        }
        Ok(request)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignedRevocation {
    pub(crate) request: RevocationRequest,
    pub(crate) signature: [u8; 64],
}

#[derive(Clone, Debug)]
struct Binding {
    known: KnownDevice,
    revoked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RevocationReceipt {
    request: RevocationRequest,
    key: [u8; 32],
    revision: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct RegisteredAuthority {
    authority: SignedAuthorityV2,
    bindings: Vec<Binding>,
    revocations: BTreeMap<OperationId, RevocationReceipt>,
}

impl RegisteredAuthority {
    pub(crate) fn bootstrap_fixture(
        organization: OrganizationId,
        initial: PrincipalId,
        policy_admin: PrincipalId,
        known: Vec<KnownDevice>,
    ) -> Result<Self> {
        if known.is_empty() || known.len() > MAX_BINDINGS {
            return Err(Refused);
        }
        let mut devices = BTreeSet::new();
        let mut credentials = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut has_initial = false;
        for binding in &known {
            if binding.organization != organization
                || !devices.insert(binding.device.clone())
                || !credentials.insert(binding.credential.clone())
                || !keys.insert(binding.key.to_bytes())
            {
                return Err(Refused);
            }
            has_initial |= binding.principal == initial;
        }
        if !has_initial {
            return Err(Refused);
        }
        Ok(Self {
            authority: SignedAuthorityV2::bootstrap(organization, initial, policy_admin)?,
            bindings: known
                .into_iter()
                .map(|known| Binding {
                    known,
                    revoked: false,
                })
                .collect(),
            revocations: BTreeMap::new(),
        })
    }

    pub(crate) fn known_device(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        device: &DeviceId,
        credential: &CredentialId,
    ) -> Result<&KnownDevice> {
        if organization != &self.authority.core.organization {
            return Err(Refused);
        }
        self.authority.core.active(principal)?;
        self.bindings
            .iter()
            .find(|binding| {
                !binding.revoked
                    && &binding.known.organization == organization
                    && &binding.known.principal == principal
                    && &binding.known.device == device
                    && &binding.known.credential == credential
            })
            .map(|binding| &binding.known)
            .ok_or(Refused)
    }

    pub(crate) fn apply_signed(&mut self, signed: &SignedRequestV2) -> Result<Outcome> {
        let request = &signed.request;
        let device = self
            .known_device(
                &request.organization,
                &request.actor,
                &request.device,
                &request.credential,
            )?
            .clone();
        if self.revocations.contains_key(&request.operation)
            || (!self.authority.receipts.contains_key(&request.operation)
                && self.authority.receipts.len() + self.revocations.len() >= MAX_OPERATIONS)
        {
            return Err(Refused);
        }
        self.authority.apply_signed(signed, &device)
    }

    pub(crate) fn apply_revocation(&mut self, signed: &SignedRevocation) -> Result<Outcome> {
        let request = &signed.request;
        let signer = self.known_device(
            &request.organization,
            &request.actor,
            &request.device,
            &request.credential,
        )?;
        signer
            .key
            .verify_strict(
                &request.canonical_bytes(),
                &Signature::from_bytes(&signed.signature),
            )
            .map_err(|_| Refused)?;
        let key = signer.key.to_bytes();
        if self.authority.receipts.contains_key(&request.operation) {
            return Err(Refused);
        }
        if let Some(receipt) = self.revocations.get(&request.operation) {
            return if receipt.request == *request && receipt.key == key {
                Ok(Outcome {
                    admission: Admission::Historical,
                    revision: receipt.revision,
                })
            } else {
                Err(Refused)
            };
        }
        if request.base_revision != self.authority.revision()
            || self.authority.receipts.len() + self.revocations.len() >= MAX_OPERATIONS
        {
            return Err(Refused);
        }
        let target = self
            .bindings
            .iter()
            .position(|binding| {
                !binding.revoked
                    && binding.known.organization == request.organization
                    && binding.known.principal == request.actor
                    && binding.known.device == request.target_device
                    && binding.known.credential == request.target_credential
            })
            .ok_or(Refused)?;
        let revision = self.authority.revision().checked_add(1).ok_or(Refused)?;
        self.bindings[target].revoked = true;
        self.authority.core.revision = revision;
        self.revocations.insert(
            request.operation.clone(),
            RevocationReceipt {
                request: request.clone(),
                key,
                revision,
            },
        );
        Ok(Outcome {
            admission: Admission::NewlyAdmitted,
            revision,
        })
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

    pub(crate) fn can_add_own_device(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> bool {
        self.authority.can_add_own_device(organization, principal)
    }

    pub(crate) fn can_manage_enrollment_policy(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> bool {
        self.authority
            .can_manage_enrollment_policy(organization, principal)
    }

    pub(crate) fn validate_additional_candidate(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        device: &DeviceId,
        credential: &CredentialId,
        key: [u8; 32],
    ) -> Result<()> {
        if !self.can_add_own_device(organization, principal) || self.bindings.len() >= MAX_BINDINGS
        {
            return Err(Refused);
        }
        KnownDevice::new(
            organization.clone(),
            principal.clone(),
            device.clone(),
            credential.clone(),
            key,
        )?;
        if self.bindings.iter().any(|binding| {
            binding.known.device == *device
                || binding.known.credential == *credential
                || binding.known.key.to_bytes() == key
        }) {
            return Err(Refused);
        }
        Ok(())
    }

    pub(crate) fn operation_is_known(&self, operation: &OperationId) -> bool {
        self.authority.receipts.contains_key(operation) || self.revocations.contains_key(operation)
    }
}
