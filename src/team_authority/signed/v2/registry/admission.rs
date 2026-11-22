use super::*;
use crate::approval::durable::{AdditionalWitness, PublicPasskeyState};
use crate::approval::{AssertionContext, EnrollmentAssertionVerifier};
use crate::durable::pending::coordinator::additional::{
    additional_context_digest, existing_client_message, ExistingClientEndorsement,
};
use crate::durable::registered_assertion::registered_policy_commitment;
use crate::durable::Checkpoint;
use crate::enrollment_proof::{decode_hex, Challenge, Proof};

pub(crate) const ADMISSION_DOMAIN: &[u8] = b"vivac.device.additional-admission.v1\0";
const PROOF_DOMAIN: &[u8] = b"vivac.device.enrollment-proof.v1\0";
pub(crate) const ADMISSION_HEADER: usize = ADMISSION_DOMAIN.len() + 251 + 64 + 52 + 64 + 48 + 8 + 2;
pub(crate) const MAX_ADMISSION_BYTES: usize = ADMISSION_HEADER + 16 * 1024;
pub(crate) const MAX_ENROLLMENTS: usize = 64;

#[derive(Clone, Debug)]
pub(crate) struct FixturePasskeyAssociation {
    pub(crate) device: DeviceId,
    pub(crate) credential: CredentialId,
    pub(crate) state: PublicPasskeyState,
}

#[derive(Clone)]
pub(crate) struct AdmissionRecord {
    pub(crate) proof: Proof,
    pub(crate) endorsement: ExistingClientEndorsement,
    pub(crate) witness: AdditionalWitness,
    pub(crate) accepted_at: u64,
}

impl AdmissionRecord {
    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>> {
        if self.proof.status != "possession_only"
            || self.proof.authority != "not_checked"
            || self.proof.challenge_use != "not_checked"
        {
            return Err(Refused);
        }
        let assertion = self.witness.assertion();
        if assertion.is_empty() || assertion.len() > 16 * 1024 {
            return Err(Refused);
        }
        let mut bytes = Vec::with_capacity(ADMISSION_HEADER + assertion.len());
        bytes.extend_from_slice(ADMISSION_DOMAIN);
        bytes.extend_from_slice(
            &self
                .proof
                .challenge
                .canonical_bytes()
                .map_err(|_| Refused)?,
        );
        bytes.extend_from_slice(&decode_hex::<64>(&self.proof.signature).map_err(|_| Refused)?);
        bytes.extend_from_slice(self.endorsement.device.as_str().as_bytes());
        bytes.extend_from_slice(self.endorsement.credential.as_str().as_bytes());
        bytes.extend_from_slice(&self.endorsement.signature);
        let context = self.witness.context();
        bytes.extend_from_slice(&context.nonce);
        bytes.extend_from_slice(&context.issued_at.to_be_bytes());
        bytes.extend_from_slice(&context.expires_at.to_be_bytes());
        bytes.extend_from_slice(&self.accepted_at.to_be_bytes());
        bytes.extend_from_slice(&(assertion.len() as u16).to_be_bytes());
        bytes.extend_from_slice(assertion);
        Ok(bytes)
    }

    pub(crate) fn parse_canonical(bytes: &[u8]) -> Result<Self> {
        if !(ADMISSION_HEADER + 1..=MAX_ADMISSION_BYTES).contains(&bytes.len())
            || !bytes.starts_with(ADMISSION_DOMAIN)
        {
            return Err(Refused);
        }
        let mut input = &bytes[ADMISSION_DOMAIN.len()..];
        let challenge = parse_challenge(take(&mut input, 251)?)?;
        let signature = hex(take(&mut input, 64)?);
        let device = DeviceId::parse(text(take(&mut input, 26)?)?)?;
        let credential = CredentialId::parse(text(take(&mut input, 26)?)?)?;
        let endorsement_signature = take(&mut input, 64)?.try_into().map_err(|_| Refused)?;
        let nonce = take(&mut input, 32)?.try_into().map_err(|_| Refused)?;
        let issued_at = integer(&mut input)?;
        let expires_at = integer(&mut input)?;
        let accepted_at = integer(&mut input)?;
        let length =
            u16::from_be_bytes(take(&mut input, 2)?.try_into().map_err(|_| Refused)?) as usize;
        if length == 0 || length > 16 * 1024 || input.len() != length {
            return Err(Refused);
        }
        let witness = AdditionalWitness::from_untrusted(
            AssertionContext {
                nonce,
                issued_at,
                expires_at,
            },
            input,
        )
        .map_err(|_| Refused)?;
        let record = Self {
            proof: Proof {
                challenge,
                signature,
                status: "possession_only".into(),
                authority: "not_checked".into(),
                challenge_use: "not_checked".into(),
            },
            endorsement: ExistingClientEndorsement {
                device,
                credential,
                signature: endorsement_signature,
            },
            witness,
            accepted_at,
        };
        if record.canonical_bytes()? != bytes {
            return Err(Refused);
        }
        Ok(record)
    }
}

pub(crate) enum AdmissionRequest {
    Policy(SignedRequestV2),
    Revoke(SignedRevocation),
    Enroll(Box<AdmissionRecord>),
}

impl AdmissionRequest {
    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Policy(value) => Ok(value.request.canonical_bytes()),
            Self::Revoke(value) => Ok(value.request.canonical_bytes()),
            Self::Enroll(value) => value.canonical_bytes(),
        }
    }
    pub(crate) fn signature(&self) -> &[u8; 64] {
        match self {
            Self::Policy(value) => &value.signature,
            Self::Revoke(value) => &value.signature,
            Self::Enroll(value) => &value.endorsement.signature,
        }
    }
    pub(crate) fn operation(&self) -> &str {
        match self {
            Self::Policy(value) => value.request.operation.as_str(),
            Self::Revoke(value) => value.request.operation.as_str(),
            Self::Enroll(value) => &value.proof.challenge.operation_id,
        }
    }
    pub(crate) fn parse_canonical(bytes: &[u8], signature: [u8; 64]) -> Result<Self> {
        let request = if bytes.len() == REVOCATION_BYTES {
            Self::Revoke(SignedRevocation {
                request: RevocationRequest::parse_canonical(bytes)?,
                signature,
            })
        } else if bytes.len() <= MAX_ENVELOPE_BYTES {
            Self::Policy(SignedRequestV2 {
                request: DeviceRequestV2::parse_canonical(bytes)?,
                signature,
            })
        } else {
            let value = AdmissionRecord::parse_canonical(bytes)?;
            if value.endorsement.signature != signature {
                return Err(Refused);
            }
            Self::Enroll(Box::new(value))
        };
        if request.canonical_bytes()? != bytes {
            return Err(Refused);
        }
        Ok(request)
    }
}

struct EnrollmentReceipt {
    bytes: Vec<u8>,
    key: [u8; 32],
    predecessor: Checkpoint,
    revision: u64,
}

pub(crate) struct AdmissionAuthority {
    registry: RegisteredAuthority,
    passkeys: BTreeMap<(DeviceId, CredentialId), PublicPasskeyState>,
    enrollments: BTreeMap<OperationId, EnrollmentReceipt>,
    candidate_nonces: BTreeSet<[u8; 32]>,
    ceremony_nonces: BTreeSet<[u8; 32]>,
}

impl AdmissionAuthority {
    pub(crate) fn bootstrap_fixture(
        organization: OrganizationId,
        initial: PrincipalId,
        policy_admin: PrincipalId,
        known: Vec<KnownDevice>,
        associations: &[FixturePasskeyAssociation],
    ) -> Result<Self> {
        if associations.is_empty() || associations.len() > MAX_BINDINGS {
            return Err(Refused);
        }
        let registry = RegisteredAuthority::bootstrap_fixture(
            organization.clone(),
            initial.clone(),
            policy_admin,
            known,
        )?;
        let mut passkeys = BTreeMap::new();
        let mut credentials = BTreeSet::new();
        let mut has_initial = false;
        for association in associations {
            let state = &association.state;
            if state.organization() != &organization
                || state.generation() != 0
                || !credentials.insert(state.credential_id().to_vec())
            {
                return Err(Refused);
            }
            if !registry.bindings.iter().any(|binding| {
                binding.known.organization == organization
                    && binding.known.principal == *state.principal()
                    && binding.known.device == association.device
                    && binding.known.credential == association.credential
            }) {
                return Err(Refused);
            }
            has_initial |= state.principal() == &initial;
            if passkeys
                .insert(
                    (association.device.clone(), association.credential.clone()),
                    state.clone(),
                )
                .is_some()
            {
                return Err(Refused);
            }
        }
        if !has_initial {
            return Err(Refused);
        }
        Ok(Self {
            registry,
            passkeys,
            enrollments: BTreeMap::new(),
            candidate_nonces: BTreeSet::new(),
            ceremony_nonces: BTreeSet::new(),
        })
    }
    pub(crate) fn revision(&self) -> u64 {
        self.registry.revision()
    }
    pub(crate) fn allows(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        project: &ProjectId,
        permission: ProjectPermission,
    ) -> bool {
        self.registry
            .allows(organization, principal, project, permission)
    }
    pub(crate) fn known_device(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        device: &DeviceId,
        credential: &CredentialId,
    ) -> Result<&KnownDevice> {
        self.registry
            .known_device(organization, principal, device, credential)
    }
    pub(crate) fn validate_additional_candidate(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        device: &DeviceId,
        credential: &CredentialId,
        key: [u8; 32],
    ) -> Result<()> {
        if self.enrollments.len() >= MAX_ENROLLMENTS {
            return Err(Refused);
        }
        self.registry.validate_additional_candidate(
            organization,
            principal,
            device,
            credential,
            key,
        )
    }
    pub(crate) fn operation_is_known(&self, operation: &OperationId) -> bool {
        self.registry.operation_is_known(operation) || self.enrollments.contains_key(operation)
    }
    pub(crate) fn has_enrollment(&self, operation: &OperationId) -> bool {
        self.enrollments.contains_key(operation)
    }
    pub(crate) fn nonce_is_consumed(&self, nonce: [u8; 32]) -> bool {
        self.candidate_nonces.contains(&nonce)
    }
    pub(crate) fn passkey_state(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        device: &DeviceId,
        credential: &CredentialId,
    ) -> Result<&PublicPasskeyState> {
        self.known_device(organization, principal, device, credential)?;
        self.passkeys
            .get(&(device.clone(), credential.clone()))
            .filter(|state| state.organization() == organization && state.principal() == principal)
            .ok_or(Refused)
    }
    fn count(&self) -> usize {
        self.registry.authority.receipts.len()
            + self.registry.revocations.len()
            + self.enrollments.len()
    }
    pub(crate) fn reconcile_additional_enrollment(
        &self,
        proof: &Proof,
        endorsement: &ExistingClientEndorsement,
    ) -> Result<Outcome> {
        let organization = OrganizationId::parse(&proof.challenge.organization_id)?;
        let principal = PrincipalId::parse(&proof.challenge.principal_id)?;
        let known = self.known_device(
            &organization,
            &principal,
            &endorsement.device,
            &endorsement.credential,
        )?;
        let operation = OperationId::parse(&proof.challenge.operation_id)?;
        let receipt = self.enrollments.get(&operation).ok_or(Refused)?;
        let record = AdmissionRecord::parse_canonical(&receipt.bytes)?;
        if known.key.to_bytes() != receipt.key
            || proof.challenge.canonical_bytes().map_err(|_| Refused)?
                != record
                    .proof
                    .challenge
                    .canonical_bytes()
                    .map_err(|_| Refused)?
            || proof.signature != record.proof.signature
            || endorsement.device != record.endorsement.device
            || endorsement.credential != record.endorsement.credential
            || endorsement.signature != record.endorsement.signature
        {
            return Err(Refused);
        }
        proof.verify(record.accepted_at).map_err(|_| Refused)?;
        let policy = registered_policy_commitment(&organization, &principal, receipt.predecessor);
        let message = existing_client_message(
            &organization,
            &principal,
            &endorsement.device,
            &endorsement.credential,
            receipt.predecessor,
            policy,
            &proof.challenge,
        )
        .map_err(|_| Refused)?;
        known
            .key
            .verify_strict(&message, &Signature::from_bytes(&endorsement.signature))
            .map_err(|_| Refused)?;
        Ok(Outcome {
            admission: Admission::Historical,
            revision: receipt.revision,
        })
    }
    pub(crate) fn apply_at(
        &mut self,
        request: &AdmissionRequest,
        checkpoint: Checkpoint,
        verifier: &EnrollmentAssertionVerifier,
    ) -> Result<Outcome> {
        match request {
            AdmissionRequest::Policy(signed) => {
                if self.enrollments.contains_key(&signed.request.operation)
                    || (!self
                        .registry
                        .authority
                        .receipts
                        .contains_key(&signed.request.operation)
                        && self.count() >= MAX_OPERATIONS)
                {
                    return Err(Refused);
                }
                self.registry.apply_signed(signed)
            }
            AdmissionRequest::Revoke(signed) => {
                if self.enrollments.contains_key(&signed.request.operation)
                    || (!self
                        .registry
                        .revocations
                        .contains_key(&signed.request.operation)
                        && self.count() >= MAX_OPERATIONS)
                {
                    return Err(Refused);
                }
                self.registry.apply_revocation(signed)
            }
            AdmissionRequest::Enroll(record) => self.apply_enrollment(record, checkpoint, verifier),
        }
    }
    fn apply_enrollment(
        &mut self,
        record: &AdmissionRecord,
        checkpoint: Checkpoint,
        verifier: &EnrollmentAssertionVerifier,
    ) -> Result<Outcome> {
        let challenge = &record.proof.challenge;
        let organization = OrganizationId::parse(&challenge.organization_id)?;
        let principal = PrincipalId::parse(&challenge.principal_id)?;
        let operation = OperationId::parse(&challenge.operation_id)?;
        let known = self.known_device(
            &organization,
            &principal,
            &record.endorsement.device,
            &record.endorsement.credential,
        )?;
        let key = known.key.to_bytes();
        let bytes = record.canonical_bytes()?;
        if let Some(receipt) = self.enrollments.get(&operation) {
            if receipt.bytes != bytes || receipt.key != key {
                return Err(Refused);
            }
            let policy =
                registered_policy_commitment(&organization, &principal, receipt.predecessor);
            let message = existing_client_message(
                &organization,
                &principal,
                &record.endorsement.device,
                &record.endorsement.credential,
                receipt.predecessor,
                policy,
                challenge,
            )
            .map_err(|_| Refused)?;
            known
                .key
                .verify_strict(
                    &message,
                    &Signature::from_bytes(&record.endorsement.signature),
                )
                .map_err(|_| Refused)?;
            return Ok(Outcome {
                admission: Admission::Historical,
                revision: receipt.revision,
            });
        }
        if self.registry.operation_is_known(&operation)
            || self.count() >= MAX_OPERATIONS
            || self.enrollments.len() >= MAX_ENROLLMENTS
        {
            return Err(Refused);
        }
        record
            .proof
            .verify(record.accepted_at)
            .map_err(|_| Refused)?;
        let candidate_nonce = decode_hex::<32>(&challenge.nonce).map_err(|_| Refused)?;
        let ceremony_nonce = record.witness.context().nonce;
        if self.candidate_nonces.contains(&candidate_nonce)
            || self.ceremony_nonces.contains(&ceremony_nonce)
        {
            return Err(Refused);
        }
        let (context, state) = self.check_record_context(record, checkpoint)?;
        let updated = verifier
            .replay_recorded_additional(
                &record.proof,
                &state,
                &record.witness,
                context,
                record.accepted_at,
            )
            .map_err(|_| Refused)?;
        let candidate = KnownDevice::new(
            organization,
            principal,
            DeviceId::parse(&challenge.device_id)?,
            CredentialId::parse(&challenge.credential_id)?,
            decode_hex::<32>(&challenge.public_key).map_err(|_| Refused)?,
        )?;
        let revision = self.revision().checked_add(1).ok_or(Refused)?;
        self.registry.bindings.push(Binding {
            known: candidate,
            revoked: false,
        });
        self.registry.authority.core.revision = revision;
        self.passkeys.insert(
            (
                record.endorsement.device.clone(),
                record.endorsement.credential.clone(),
            ),
            updated,
        );
        self.candidate_nonces.insert(candidate_nonce);
        self.ceremony_nonces.insert(ceremony_nonce);
        self.enrollments.insert(
            operation,
            EnrollmentReceipt {
                bytes,
                key,
                predecessor: checkpoint,
                revision,
            },
        );
        Ok(Outcome {
            admission: Admission::NewlyAdmitted,
            revision,
        })
    }
    pub(crate) fn check_record_context(
        &self,
        record: &AdmissionRecord,
        checkpoint: Checkpoint,
    ) -> Result<([u8; 32], PublicPasskeyState)> {
        self.check_candidate_context(&record.proof, &record.endorsement, checkpoint)
    }
    pub(crate) fn check_candidate_context(
        &self,
        proof: &Proof,
        endorsement: &ExistingClientEndorsement,
        checkpoint: Checkpoint,
    ) -> Result<([u8; 32], PublicPasskeyState)> {
        let challenge = &proof.challenge;
        let organization = OrganizationId::parse(&challenge.organization_id)?;
        let principal = PrincipalId::parse(&challenge.principal_id)?;
        if checkpoint.revision != self.revision() || challenge.base_revision != checkpoint.revision
        {
            return Err(Refused);
        }
        self.validate_additional_candidate(
            &organization,
            &principal,
            &DeviceId::parse(&challenge.device_id)?,
            &CredentialId::parse(&challenge.credential_id)?,
            decode_hex::<32>(&challenge.public_key).map_err(|_| Refused)?,
        )?;
        let known = self.known_device(
            &organization,
            &principal,
            &endorsement.device,
            &endorsement.credential,
        )?;
        let policy = registered_policy_commitment(&organization, &principal, checkpoint);
        let message = existing_client_message(
            &organization,
            &principal,
            &endorsement.device,
            &endorsement.credential,
            checkpoint,
            policy,
            challenge,
        )
        .map_err(|_| Refused)?;
        known
            .key
            .verify_strict(&message, &Signature::from_bytes(&endorsement.signature))
            .map_err(|_| Refused)?;
        let context = additional_context_digest(
            &organization,
            &principal,
            checkpoint,
            policy,
            challenge,
            endorsement,
        )
        .map_err(|_| Refused)?;
        let state = self
            .passkey_state(
                &organization,
                &principal,
                &endorsement.device,
                &endorsement.credential,
            )?
            .clone();
        Ok((context, state))
    }
}

fn take<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    let value = input.get(..length).ok_or(Refused)?;
    *input = &input[length..];
    Ok(value)
}
fn integer(input: &mut &[u8]) -> Result<u64> {
    Ok(u64::from_be_bytes(
        take(input, 8)?.try_into().map_err(|_| Refused)?,
    ))
}
fn text(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| Refused)
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn parse_challenge(bytes: &[u8]) -> Result<Challenge> {
    if bytes.len() != 251 || !bytes.starts_with(PROOF_DOMAIN) {
        return Err(Refused);
    }
    let mut input = &bytes[PROOF_DOMAIN.len()..];
    let organization_id = OrganizationId::parse(text(take(&mut input, 26)?)?)?
        .as_str()
        .to_owned();
    let principal_id = PrincipalId::parse(text(take(&mut input, 26)?)?)?
        .as_str()
        .to_owned();
    let device_id = DeviceId::parse(text(take(&mut input, 26)?)?)?
        .as_str()
        .to_owned();
    let credential_id = CredentialId::parse(text(take(&mut input, 26)?)?)?
        .as_str()
        .to_owned();
    let public_key = hex(take(&mut input, 32)?);
    let operation_id = OperationId::parse(text(take(&mut input, 26)?)?)?
        .as_str()
        .to_owned();
    let base_revision = integer(&mut input)?;
    let nonce = hex(take(&mut input, 32)?);
    let issued_at = integer(&mut input)?;
    let expires_at = integer(&mut input)?;
    let challenge = Challenge {
        version: 1,
        organization_id,
        principal_id,
        device_id,
        credential_id,
        public_key,
        operation_id,
        base_revision,
        nonce,
        issued_at,
        expires_at,
    };
    if !input.is_empty() || challenge.canonical_bytes().map_err(|_| Refused)? != bytes {
        return Err(Refused);
    }
    Ok(challenge)
}

#[cfg(test)]
mod tests;
