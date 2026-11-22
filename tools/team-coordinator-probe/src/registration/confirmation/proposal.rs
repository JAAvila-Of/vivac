use super::*;

const MAX_DRAFT: usize = 8 * 1024;
const MAX_PROPOSAL: usize = 64 * 1024;
const PROFILE: &str = "initial-proposal-probe-v1";
const DRAFT_DOMAIN: &[u8] = b"vivac.bootstrap.draft-probe.v1\0";
const PROPOSAL_DOMAIN: &[u8] = b"vivac.bootstrap.proposal-probe.v1\0";
const ACCEPTANCE_PROFILE: &str = "initial-acceptance-probe-v1";
const ACCEPTANCE_DRAFT_DOMAIN: &[u8] = b"vivac.bootstrap.acceptance-draft-probe.v1\0";
const ACCEPTANCE_PROPOSAL_DOMAIN: &[u8] = b"vivac.bootstrap.acceptance-proposal-probe.v1\0";

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct DraftInput {
    version: u64,
    profile: String,
    purpose: String,
    candidate: Proof,
    coordinator: CoordinatorInput,
    webauthn: WebAuthnInput,
    grants: GrantInput,
    #[serde(
        default,
        deserialize_with = "present_acceptance",
        skip_serializing_if = "Option::is_none"
    )]
    acceptance: Option<AcceptanceInput>,
    project_grants: Vec<String>,
    governance_grants: Vec<String>,
    recovery: String,
    content_key_custody: String,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct CoordinatorInput {
    endpoint: String,
    spki_sha256: String,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct WebAuthnInput {
    rp_id: String,
    origin: String,
    registration_profile: String,
    user_verification: String,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct GrantInput {
    capabilities: Vec<String>,
    delegation: String,
    enrollment_policy_administrator: bool,
    own_additional_device: bool,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceInput {
    organization_authority_id: String,
    organization_authority_public_key: String,
    principal_acceptance: String,
}

fn present_acceptance<'de, D>(deserializer: D) -> Result<Option<AcceptanceInput>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    AcceptanceInput::deserialize(deserializer).map(Some)
}

fn draft_domain(value: &DraftInput) -> RegistrationResult<&'static [u8]> {
    match (value.version, value.profile.as_str()) {
        (1, PROFILE) => Ok(DRAFT_DOMAIN),
        (2, ACCEPTANCE_PROFILE) => Ok(ACCEPTANCE_DRAFT_DOMAIN),
        _ => Err(RegistrationError::Refused),
    }
}

fn proposal_domain(value: &DraftInput) -> RegistrationResult<&'static [u8]> {
    match (value.version, value.profile.as_str()) {
        (1, PROFILE) => Ok(PROPOSAL_DOMAIN),
        (2, ACCEPTANCE_PROFILE) => Ok(ACCEPTANCE_PROPOSAL_DOMAIN),
        _ => Err(RegistrationError::Refused),
    }
}

// Parsing public draft data creates no registration or acceptance capability.
pub(crate) struct Draft {
    value: DraftInput,
    canonical: Vec<u8>,
    commitment: [u8; 32],
}

impl Draft {
    pub(crate) fn parse(raw: &[u8]) -> RegistrationResult<Self> {
        Self::parse_using(raw, Runtime::default())
    }

    fn parse_using(raw: &[u8], runtime: Runtime) -> RegistrationResult<Self> {
        if raw.is_empty() || raw.len() > MAX_DRAFT {
            return Err(RegistrationError::Refused);
        }
        let value: DraftInput =
            serde_json::from_slice(raw).map_err(|_| RegistrationError::Refused)?;
        validate_draft(&value, runtime)?;
        let canonical = serde_json::to_vec(&value).map_err(|_| RegistrationError::Refused)?;
        if canonical.len() > MAX_DRAFT {
            return Err(RegistrationError::Refused);
        }
        Ok(Self {
            commitment: hash(draft_domain(&value)?, &canonical),
            canonical,
            value,
        })
    }

    pub(crate) fn proof(&self) -> &Proof {
        &self.value.candidate
    }

    pub(crate) fn commitment(&self) -> [u8; 32] {
        self.commitment
    }

    pub(crate) fn canonical(&self) -> &[u8] {
        &self.canonical
    }

    fn check(&self, runtime: Runtime) -> RegistrationResult<()> {
        validate_draft(&self.value, runtime)?;
        if self.canonical.len() > MAX_DRAFT
            || serde_json::to_vec(&self.value).map_err(|_| RegistrationError::Refused)?
                != self.canonical
            || hash(draft_domain(&self.value)?, &self.canonical) != self.commitment
        {
            return Err(RegistrationError::Refused);
        }
        Ok(())
    }
}

fn validate_draft(value: &DraftInput, runtime: Runtime) -> RegistrationResult<()> {
    draft_domain(value)?;
    match (value.version, value.acceptance.as_ref()) {
        (1, None) => {}
        (2, Some(acceptance)) => {
            if !crate::enrollment_proof::canonical_identifier(&acceptance.organization_authority_id)
                || acceptance.principal_acceptance != "candidate-signing-key"
            {
                return Err(RegistrationError::Refused);
            }
            let bytes = crate::enrollment_proof::decode_hex::<32>(
                &acceptance.organization_authority_public_key,
            )
            .map_err(|_| RegistrationError::Refused)?;
            let key = ed25519_dalek::VerifyingKey::from_bytes(&bytes)
                .map_err(|_| RegistrationError::Refused)?;
            if key.is_weak() {
                return Err(RegistrationError::Refused);
            }
        }
        _ => return Err(RegistrationError::Refused),
    }
    value
        .candidate
        .verify((runtime.clock)()?)
        .map_err(|_| RegistrationError::Refused)?;
    if value.purpose != "initial-organization"
        || value.candidate.challenge.base_revision != 0
        || value.webauthn.registration_profile != "passki-0.5-registration-probe-v1"
        || value.webauthn.user_verification != "required"
        || value.coordinator.endpoint != value.webauthn.origin
        || value.grants.delegation != "organization-model-v2"
        || value.grants.capabilities.len() > 4
        || !value.project_grants.is_empty()
        || !value.governance_grants.is_empty()
        || value.recovery != "not-established"
        || value.content_key_custody != "not-established"
    {
        return Err(RegistrationError::Refused);
    }
    crate::enrollment_proof::decode_hex::<32>(&value.coordinator.spki_sha256)
        .map_err(|_| RegistrationError::Refused)?;
    let verifier = RegistrationVerifier::new(&value.webauthn.rp_id, &value.webauthn.origin)?;
    if verifier.origin != value.webauthn.origin {
        return Err(RegistrationError::Refused);
    }
    for (index, capability) in value.grants.capabilities.iter().enumerate() {
        if !matches!(
            capability.as_str(),
            "grant-project-access"
                | "manage-members"
                | "manage-organization-capabilities"
                | "manage-projects"
        ) || (index > 0 && value.grants.capabilities[index - 1].as_str() >= capability.as_str())
        {
            return Err(RegistrationError::Refused);
        }
    }
    Ok(())
}

pub(crate) struct ReadyProposal {
    draft: Draft,
    reserved: ReservedConfirmation,
    packet: Vec<u8>,
    binding: ConfirmationBinding,
}

impl ReadyProposal {
    pub(crate) fn packet(&self) -> &[u8] {
        &self.packet
    }

    pub(crate) fn commitment(&self) -> [u8; 32] {
        self.binding.proposal
    }

    pub(crate) fn context(&self) -> ConfirmationContext {
        self.binding.context
    }
}

pub(crate) struct PendingProposal {
    draft: Draft,
    pending: PendingConfirmation,
    packet: Vec<u8>,
    binding: ConfirmationBinding,
}

impl PendingProposal {
    pub(crate) fn packet(&self) -> &[u8] {
        &self.packet
    }

    pub(crate) fn commitment(&self) -> [u8; 32] {
        self.binding.proposal
    }
}

// This result retains possession evidence only; it cannot activate authority.
pub(crate) struct ConfirmedProposal {
    packet: Vec<u8>,
    confirmation: PossessionConfirmation,
}

impl ConfirmedProposal {
    pub(crate) fn packet(&self) -> &[u8] {
        &self.packet
    }

    pub(crate) fn confirmation(&self) -> &PossessionConfirmation {
        &self.confirmation
    }
}

impl RegistrationVerifier {
    pub(crate) fn prepare_proposal(
        &self,
        draft: Draft,
        registration: UnconfirmedRegistration,
    ) -> RegistrationResult<ReadyProposal> {
        self.prepare_proposal_using(draft, registration, Runtime::default())
    }

    fn prepare_proposal_using(
        &self,
        draft: Draft,
        registration: UnconfirmedRegistration,
        runtime: Runtime,
    ) -> RegistrationResult<ReadyProposal> {
        draft.check(runtime)?;
        if draft.value.webauthn.rp_id != self.rp || draft.value.webauthn.origin != self.origin {
            return Err(RegistrationError::Refused);
        }
        let (context, reserved) = self.reserve_confirmation_using(
            registration,
            draft.proof(),
            draft.commitment(),
            runtime,
        )?;
        let packet = packet_bytes(&draft, &reserved.registration, context)?;
        let binding = ConfirmationBinding {
            proposal: hash(proposal_domain(&draft.value)?, &packet),
            registration: reserved.registration.commitment,
            context,
        };
        check_packet(&draft, &reserved.registration, &packet, binding, runtime)?;
        self.check_confirmation_scope(
            &reserved.registration,
            context,
            draft.proof(),
            draft.commitment(),
            runtime,
        )?;
        Ok(ReadyProposal {
            draft,
            reserved,
            packet,
            binding,
        })
    }

    pub(crate) fn start_proposal(
        &self,
        ready: ReadyProposal,
    ) -> RegistrationResult<(FinalAssertionRequest, PendingProposal)> {
        self.start_proposal_using(ready, Runtime::default())
    }

    fn start_proposal_using(
        &self,
        ready: ReadyProposal,
        runtime: Runtime,
    ) -> RegistrationResult<(FinalAssertionRequest, PendingProposal)> {
        check_packet(
            &ready.draft,
            &ready.reserved.registration,
            &ready.packet,
            ready.binding,
            runtime,
        )?;
        let (request, pending) = self.start_confirmation_using(
            ready.reserved,
            ready.draft.proof(),
            ready.draft.commitment(),
            ready.binding,
            runtime,
            authentication_factory,
        )?;
        Ok((
            request,
            PendingProposal {
                draft: ready.draft,
                pending,
                packet: ready.packet,
                binding: ready.binding,
            },
        ))
    }

    pub(crate) fn finish_proposal(
        &self,
        pending: PendingProposal,
        device_signature: &[u8],
        assertion: &[u8],
    ) -> RegistrationResult<ConfirmedProposal> {
        self.finish_proposal_using(pending, device_signature, assertion, Runtime::default())
    }

    fn finish_proposal_using(
        &self,
        pending: PendingProposal,
        device_signature: &[u8],
        assertion: &[u8],
        runtime: Runtime,
    ) -> RegistrationResult<ConfirmedProposal> {
        check_packet(
            &pending.draft,
            &pending.pending.registration,
            &pending.packet,
            pending.binding,
            runtime,
        )?;
        let confirmation = self.finish_confirmation_using(
            pending.pending,
            pending.draft.proof(),
            pending.draft.commitment(),
            pending.binding,
            FinalEvidence {
                device_signature,
                assertion,
            },
            runtime,
        )?;
        Ok(ConfirmedProposal {
            packet: pending.packet,
            confirmation,
        })
    }
}

#[derive(serde::Serialize)]
struct PublicContext {
    nonce: String,
    issued_at: u64,
    expires_at: u64,
}

impl PublicContext {
    fn new(nonce: &[u8], issued_at: u64, expires_at: u64) -> Self {
        Self {
            nonce: hex(nonce),
            issued_at,
            expires_at,
        }
    }
}

#[derive(serde::Serialize)]
struct PublicDescriptor<'a> {
    aaguid: String,
    counter: u32,
    backup_eligible: bool,
    backup_state: bool,
    algorithm: i32,
    public_key: String,
    credential: String,
    transports: &'a [String],
    attachment: Option<&'a str>,
    opaque_user_handle: String,
}

#[derive(serde::Serialize)]
struct PublicRegistration<'a> {
    commitment: String,
    context: PublicContext,
    descriptor: PublicDescriptor<'a>,
    response: String,
}

#[derive(serde::Serialize)]
struct PublicProposal<'a> {
    version: u64,
    profile: &'a str,
    draft: &'a DraftInput,
    draft_commitment: String,
    registration: PublicRegistration<'a>,
    context: PublicContext,
}

fn packet_bytes(
    draft: &Draft,
    registration: &UnconfirmedRegistration,
    context: ConfirmationContext,
) -> RegistrationResult<Vec<u8>> {
    if registration.response.len() > MAX_RESPONSE {
        return Err(RegistrationError::Refused);
    }
    let descriptor = &registration.descriptor;
    let packet = PublicProposal {
        version: draft.value.version,
        profile: &draft.value.profile,
        draft: &draft.value,
        draft_commitment: hex(&draft.commitment),
        registration: PublicRegistration {
            commitment: hex(&registration.commitment),
            context: PublicContext::new(
                &registration.context.nonce,
                registration.context.issued_at,
                registration.context.expires_at,
            ),
            descriptor: PublicDescriptor {
                aaguid: hex(&descriptor.aaguid),
                counter: descriptor.counter,
                backup_eligible: descriptor.backup_eligible,
                backup_state: descriptor.backup_state,
                algorithm: descriptor.algorithm,
                public_key: encode(&descriptor.public_key),
                credential: encode(&descriptor.credential),
                transports: &descriptor.transports,
                attachment: descriptor.attachment.as_deref(),
                opaque_user_handle: encode(&descriptor.opaque_user_handle),
            },
            response: encode(&registration.response),
        },
        context: PublicContext::new(&context.nonce, context.issued_at, context.expires_at),
    };
    let bytes = serde_json::to_vec(&packet).map_err(|_| RegistrationError::Refused)?;
    if bytes.len() > MAX_PROPOSAL {
        return Err(RegistrationError::Refused);
    }
    Ok(bytes)
}

fn check_packet(
    draft: &Draft,
    registration: &UnconfirmedRegistration,
    packet: &[u8],
    binding: ConfirmationBinding,
    runtime: Runtime,
) -> RegistrationResult<()> {
    draft.check(runtime)?;
    if packet.len() > MAX_PROPOSAL
        || binding.registration != registration.commitment
        || hash(proposal_domain(&draft.value)?, packet) != binding.proposal
        || packet_bytes(draft, registration, binding.context)? != packet
    {
        return Err(RegistrationError::Refused);
    }
    Ok(())
}

fn hash(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut result = Sha256::new();
    result.update(domain);
    result.update(bytes);
    result.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) mod acceptance;

#[cfg(test)]
mod tests;
