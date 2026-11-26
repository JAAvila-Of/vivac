use super::*;

const ORGANIZATION_DOMAIN: &[u8] = b"vivac.bootstrap.organization-acceptance-probe.v1\0";
const PRINCIPAL_DOMAIN: &[u8] = b"vivac.bootstrap.principal-acceptance-probe.v1\0";

// These expectations are explicit fixture data, not a production trust anchor.
pub(crate) struct FixtureSigner {
    id: String,
    key: [u8; 32],
}

impl FixtureSigner {
    pub(crate) fn new(id: &str, key: [u8; 32]) -> RegistrationResult<Self> {
        check_signer(id, &key)?;
        Ok(Self {
            id: id.to_owned(),
            key,
        })
    }
}

fn check_signer(id: &str, bytes: &[u8; 32]) -> RegistrationResult<()> {
    if !crate::enrollment_proof::canonical_identifier(id) {
        return Err(RegistrationError::Refused);
    }
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(bytes).map_err(|_| RegistrationError::Refused)?;
    if key.is_weak() {
        return Err(RegistrationError::Refused);
    }
    Ok(())
}

pub(crate) struct FixtureExpectations {
    organization: String,
    operation: String,
    proposal: [u8; 32],
    organization_signer: FixtureSigner,
    principal_signer: FixtureSigner,
}

impl FixtureExpectations {
    pub(crate) fn new_fixture(
        organization: &str,
        operation: &str,
        proposal: [u8; 32],
        organization_signer: FixtureSigner,
        principal_signer: FixtureSigner,
    ) -> RegistrationResult<Self> {
        if !crate::enrollment_proof::canonical_identifier(organization)
            || !crate::enrollment_proof::canonical_identifier(operation)
        {
            return Err(RegistrationError::Refused);
        }
        check_signer(&organization_signer.id, &organization_signer.key)?;
        check_signer(&principal_signer.id, &principal_signer.key)?;
        Ok(Self {
            organization: organization.to_owned(),
            operation: operation.to_owned(),
            proposal,
            organization_signer,
            principal_signer,
        })
    }

    fn check(&self, view: &ProposalView<'_>) -> RegistrationResult<()> {
        if !crate::enrollment_proof::canonical_identifier(&self.organization)
            || !crate::enrollment_proof::canonical_identifier(&self.operation)
            || view.draft.value.version != 2
            || view.draft.value.profile != ACCEPTANCE_PROFILE
        {
            return Err(RegistrationError::Refused);
        }
        check_signer(&self.organization_signer.id, &self.organization_signer.key)?;
        check_signer(&self.principal_signer.id, &self.principal_signer.key)?;
        let candidate = &view.draft.proof().challenge;
        let acceptance = view
            .draft
            .value
            .acceptance
            .as_ref()
            .ok_or(RegistrationError::Refused)?;
        let organization_key = crate::enrollment_proof::decode_hex::<32>(
            &acceptance.organization_authority_public_key,
        )
        .map_err(|_| RegistrationError::Refused)?;
        let principal_key = crate::enrollment_proof::decode_hex::<32>(&candidate.public_key)
            .map_err(|_| RegistrationError::Refused)?;
        if self.organization != candidate.organization_id
            || self.operation != candidate.operation_id
            || self.proposal != view.binding.proposal
            || self.organization_signer.id != acceptance.organization_authority_id
            || self.organization_signer.key != organization_key
            || self.principal_signer.id != candidate.principal_id
            || self.principal_signer.key != principal_key
            || acceptance.principal_acceptance != "candidate-signing-key"
        {
            return Err(RegistrationError::Refused);
        }
        Ok(())
    }
}

#[derive(serde::Serialize)]
pub(crate) struct FixtureAcceptanceRequest {
    pub(crate) organization_message: String,
    pub(crate) principal_message: String,
}

pub(crate) struct PendingFixtureAcceptances {
    ready: ReadyProposal,
    expectations: FixtureExpectations,
    organization_message: Vec<u8>,
    principal_message: Vec<u8>,
}

impl PendingFixtureAcceptances {
    pub(crate) fn packet(&self) -> &[u8] {
        self.ready.packet()
    }
    pub(crate) fn commitment(&self) -> [u8; 32] {
        self.ready.commitment()
    }
}

// Verified signatures under fixture expectations do not accredit either signer.
pub(crate) struct FixtureAcceptanceEvidence {
    expectations: FixtureExpectations,
    organization_message: Vec<u8>,
    principal_message: Vec<u8>,
    organization_signature: Vec<u8>,
    principal_signature: Vec<u8>,
}

impl FixtureAcceptanceEvidence {
    pub(crate) fn organization_signature(&self) -> &[u8] {
        &self.organization_signature
    }
    pub(crate) fn principal_signature(&self) -> &[u8] {
        &self.principal_signature
    }
    pub(crate) fn organization_message(&self) -> &[u8] {
        &self.organization_message
    }
    pub(crate) fn principal_message(&self) -> &[u8] {
        &self.principal_message
    }
}

pub(crate) struct FixtureVerifiedReadyProposal {
    ready: ReadyProposal,
    evidence: FixtureAcceptanceEvidence,
}

impl FixtureVerifiedReadyProposal {
    pub(crate) fn packet(&self) -> &[u8] {
        self.ready.packet()
    }
    pub(crate) fn commitment(&self) -> [u8; 32] {
        self.ready.commitment()
    }
}

pub(crate) struct PendingFixtureProposal {
    pending: PendingProposal,
    evidence: FixtureAcceptanceEvidence,
}

impl PendingFixtureProposal {
    pub(crate) fn packet(&self) -> &[u8] {
        self.pending.packet()
    }
    pub(crate) fn commitment(&self) -> [u8; 32] {
        self.pending.commitment()
    }
}

pub(crate) struct FixtureCompleteProposal {
    confirmed: ConfirmedProposal,
    evidence: FixtureAcceptanceEvidence,
}

impl FixtureCompleteProposal {
    pub(crate) fn packet(&self) -> &[u8] {
        self.confirmed.packet()
    }
    pub(crate) fn possession(&self) -> &PossessionConfirmation {
        self.confirmed.confirmation()
    }
    pub(crate) fn acceptances(&self) -> &FixtureAcceptanceEvidence {
        &self.evidence
    }
}

struct ProposalView<'a> {
    draft: &'a Draft,
    registration: &'a UnconfirmedRegistration,
    packet: &'a [u8],
    binding: ConfirmationBinding,
}

fn ready_view(ready: &ReadyProposal) -> ProposalView<'_> {
    ProposalView {
        draft: &ready.draft,
        registration: &ready.reserved.registration,
        packet: &ready.packet,
        binding: ready.binding,
    }
}

fn pending_view(pending: &PendingProposal) -> ProposalView<'_> {
    ProposalView {
        draft: &pending.draft,
        registration: &pending.pending.registration,
        packet: &pending.packet,
        binding: pending.binding,
    }
}

fn message(domain: &[u8], view: &ProposalView<'_>, signer: &FixtureSigner) -> Vec<u8> {
    let candidate = &view.draft.proof().challenge;
    let mut bytes = domain.to_vec();
    bytes.extend_from_slice(&view.binding.proposal);
    bytes.extend_from_slice(candidate.organization_id.as_bytes());
    bytes.extend_from_slice(candidate.operation_id.as_bytes());
    bytes.extend_from_slice(signer.id.as_bytes());
    bytes.extend_from_slice(&signer.key);
    bytes.extend_from_slice(&view.binding.context.nonce);
    bytes.extend_from_slice(&view.binding.context.issued_at.to_be_bytes());
    bytes.extend_from_slice(&view.binding.context.expires_at.to_be_bytes());
    bytes
}

fn verify_signature(
    signer: &FixtureSigner,
    bytes: &[u8],
    signature: &[u8],
) -> RegistrationResult<()> {
    check_signer(&signer.id, &signer.key)?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&signer.key)
        .map_err(|_| RegistrationError::Refused)?;
    let signature =
        ed25519_dalek::Signature::from_slice(signature).map_err(|_| RegistrationError::Refused)?;
    key.verify_strict(bytes, &signature)
        .map_err(|_| RegistrationError::Refused)
}

impl RegistrationVerifier {
    fn check_fixture_scope(
        &self,
        view: &ProposalView<'_>,
        runtime: Runtime,
    ) -> RegistrationResult<()> {
        check_packet(
            view.draft,
            view.registration,
            view.packet,
            view.binding,
            runtime,
        )?;
        self.check_confirmation_scope(
            view.registration,
            view.binding.context,
            view.draft.proof(),
            view.draft.commitment(),
            runtime,
        )
    }

    fn check_fixture_pending(
        &self,
        pending: &PendingFixtureAcceptances,
        runtime: Runtime,
    ) -> RegistrationResult<()> {
        let view = ready_view(&pending.ready);
        self.check_fixture_scope(&view, runtime)?;
        pending.expectations.check(&view)?;
        if pending.organization_message
            != message(
                ORGANIZATION_DOMAIN,
                &view,
                &pending.expectations.organization_signer,
            )
            || pending.principal_message
                != message(
                    PRINCIPAL_DOMAIN,
                    &view,
                    &pending.expectations.principal_signer,
                )
        {
            return Err(RegistrationError::Refused);
        }
        Ok(())
    }

    fn check_fixture_evidence(
        &self,
        view: &ProposalView<'_>,
        evidence: &FixtureAcceptanceEvidence,
        runtime: Runtime,
    ) -> RegistrationResult<()> {
        self.check_fixture_scope(view, runtime)?;
        evidence.expectations.check(view)?;
        if evidence.organization_message
            != message(
                ORGANIZATION_DOMAIN,
                view,
                &evidence.expectations.organization_signer,
            )
            || evidence.principal_message
                != message(
                    PRINCIPAL_DOMAIN,
                    view,
                    &evidence.expectations.principal_signer,
                )
        {
            return Err(RegistrationError::Refused);
        }
        verify_signature(
            &evidence.expectations.organization_signer,
            &evidence.organization_message,
            &evidence.organization_signature,
        )?;
        verify_signature(
            &evidence.expectations.principal_signer,
            &evidence.principal_message,
            &evidence.principal_signature,
        )?;
        self.check_fixture_scope(view, runtime)
    }

    #[cfg(all(test, feature = "browser-lab", target_os = "linux"))]
    pub(crate) fn begin_fixture_acceptances(
        &self,
        ready: ReadyProposal,
        expectations: FixtureExpectations,
    ) -> RegistrationResult<(FixtureAcceptanceRequest, PendingFixtureAcceptances)> {
        self.begin_fixture_acceptances_using(ready, expectations, Runtime::default())
    }

    fn begin_fixture_acceptances_using(
        &self,
        ready: ReadyProposal,
        expectations: FixtureExpectations,
        runtime: Runtime,
    ) -> RegistrationResult<(FixtureAcceptanceRequest, PendingFixtureAcceptances)> {
        let view = ready_view(&ready);
        self.check_fixture_scope(&view, runtime)?;
        expectations.check(&view)?;
        let organization_message = message(
            ORGANIZATION_DOMAIN,
            &view,
            &expectations.organization_signer,
        );
        let principal_message = message(PRINCIPAL_DOMAIN, &view, &expectations.principal_signer);
        let request = FixtureAcceptanceRequest {
            organization_message: encode(&organization_message),
            principal_message: encode(&principal_message),
        };
        let pending = PendingFixtureAcceptances {
            ready,
            expectations,
            organization_message,
            principal_message,
        };
        self.check_fixture_pending(&pending, runtime)?;
        Ok((request, pending))
    }

    #[cfg(all(test, feature = "browser-lab", target_os = "linux"))]
    pub(crate) fn finish_fixture_acceptances(
        &self,
        pending: PendingFixtureAcceptances,
        organization_signature: &[u8],
        principal_signature: &[u8],
    ) -> RegistrationResult<FixtureVerifiedReadyProposal> {
        self.finish_fixture_acceptances_using(
            pending,
            organization_signature,
            principal_signature,
            Runtime::default(),
        )
    }

    fn finish_fixture_acceptances_using(
        &self,
        pending: PendingFixtureAcceptances,
        organization_signature: &[u8],
        principal_signature: &[u8],
        runtime: Runtime,
    ) -> RegistrationResult<FixtureVerifiedReadyProposal> {
        self.check_fixture_pending(&pending, runtime)?;
        verify_signature(
            &pending.expectations.organization_signer,
            &pending.organization_message,
            organization_signature,
        )?;
        verify_signature(
            &pending.expectations.principal_signer,
            &pending.principal_message,
            principal_signature,
        )?;
        self.check_fixture_pending(&pending, runtime)?;
        Ok(FixtureVerifiedReadyProposal {
            ready: pending.ready,
            evidence: FixtureAcceptanceEvidence {
                expectations: pending.expectations,
                organization_message: pending.organization_message,
                principal_message: pending.principal_message,
                organization_signature: organization_signature.to_vec(),
                principal_signature: principal_signature.to_vec(),
            },
        })
    }

    #[cfg(all(test, feature = "browser-lab", target_os = "linux"))]
    pub(crate) fn start_fixture_proposal(
        &self,
        verified: FixtureVerifiedReadyProposal,
    ) -> RegistrationResult<(FinalAssertionRequest, PendingFixtureProposal)> {
        self.start_fixture_proposal_using(verified, Runtime::default())
    }

    fn start_fixture_proposal_using(
        &self,
        verified: FixtureVerifiedReadyProposal,
        runtime: Runtime,
    ) -> RegistrationResult<(FinalAssertionRequest, PendingFixtureProposal)> {
        self.check_fixture_evidence(&ready_view(&verified.ready), &verified.evidence, runtime)?;
        let (request, pending) = self.start_proposal_using(verified.ready, runtime)?;
        self.check_fixture_evidence(&pending_view(&pending), &verified.evidence, runtime)?;
        Ok((
            request,
            PendingFixtureProposal {
                pending,
                evidence: verified.evidence,
            },
        ))
    }

    #[cfg(all(test, feature = "browser-lab", target_os = "linux"))]
    pub(crate) fn finish_fixture_proposal(
        &self,
        pending: PendingFixtureProposal,
        device_signature: &[u8],
        assertion: &[u8],
    ) -> RegistrationResult<FixtureCompleteProposal> {
        self.finish_fixture_proposal_using(pending, device_signature, assertion, Runtime::default())
    }

    fn finish_fixture_proposal_using(
        &self,
        pending: PendingFixtureProposal,
        device_signature: &[u8],
        assertion: &[u8],
        runtime: Runtime,
    ) -> RegistrationResult<FixtureCompleteProposal> {
        self.check_fixture_evidence(&pending_view(&pending.pending), &pending.evidence, runtime)?;
        let confirmed =
            self.finish_proposal_using(pending.pending, device_signature, assertion, runtime)?;
        Ok(FixtureCompleteProposal {
            confirmed,
            evidence: pending.evidence,
        })
    }
}

#[cfg(test)]
mod tests;
