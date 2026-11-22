use super::{additional_context_digest, existing_client_message, ExistingClientEndorsement};
use super::{Checkpoint, StoreError, StoreResult};
use crate::approval::{additional_commitment, AssertionRequest};
use crate::enrollment_proof::{decode_hex, Challenge, Proof};
use crate::team_authority::{CredentialId, DeviceId, OrganizationId, PrincipalId};
use base64ct::{Base64UrlUnpadded, Encoding};
use ed25519_dalek::{Signature, VerifyingKey};
use passki::{AuthenticationChallenge, UserVerificationRequirement};
use url::Url;

// These explicit test expectations have no admitted trust or network loader.
pub(crate) struct FixtureClientExpectation {
    pub(crate) candidate: Challenge,
    pub(crate) organization: OrganizationId,
    pub(crate) principal: PrincipalId,
    pub(crate) credential: Vec<u8>,
    pub(crate) rp_id: String,
    pub(crate) origin: String,
    pub(crate) previous_device: DeviceId,
    pub(crate) previous_credential: CredentialId,
    pub(crate) previous_public_key: [u8; 32],
    pub(crate) checkpoint: Checkpoint,
    pub(crate) policy: [u8; 32],
}

// A local reference match records selection, without proving trust or human approval.
pub(crate) struct CandidateSelection {
    candidate: [u8; 251],
    selected_at: u64,
}

impl CandidateSelection {
    pub(crate) fn from_reference(proof: &Proof, expected: &str, now: u64) -> StoreResult<Self> {
        decode_hex::<32>(expected).map_err(|_| StoreError::Refused)?;
        proof.verify(now).map_err(|_| StoreError::Refused)?;
        if expected
            != proof
                .challenge
                .candidate_reference()
                .map_err(|_| StoreError::Refused)?
        {
            return Err(StoreError::Refused);
        }
        let candidate = proof
            .challenge
            .canonical_bytes()
            .map_err(|_| StoreError::Refused)?
            .try_into()
            .map_err(|_| StoreError::Refused)?;
        Ok(Self {
            candidate,
            selected_at: now,
        })
    }
}

// Checked parameters are not evidence of human interaction or permission.
pub(crate) struct CheckedAdditionalRequest {
    candidate: Challenge,
    options: AuthenticationChallenge,
    checked_at: u64,
}

impl CheckedAdditionalRequest {
    pub(crate) fn candidate(&self) -> &Challenge {
        &self.candidate
    }

    // Consume this check and bound the browser timeout at the point of use.
    pub(crate) fn into_options(mut self, now: u64) -> StoreResult<AuthenticationChallenge> {
        if now < self.checked_at {
            return Err(StoreError::Refused);
        }
        self.candidate
            .validate(now)
            .map_err(|_| StoreError::Refused)?;
        self.options.timeout = self
            .options
            .timeout
            .min((self.candidate.expires_at - now) * 1000);
        Ok(self.options)
    }
}

pub(crate) fn check_request(
    expected: &FixtureClientExpectation,
    selection: CandidateSelection,
    proof: &Proof,
    request: AssertionRequest,
    endorsement: &ExistingClientEndorsement,
    now: u64,
) -> StoreResult<CheckedAdditionalRequest> {
    if now < selection.selected_at {
        return Err(StoreError::Refused);
    }
    expected
        .candidate
        .validate(now)
        .map_err(|_| StoreError::Refused)?;
    proof.verify(now).map_err(|_| StoreError::Refused)?;
    let canonical = expected
        .candidate
        .canonical_bytes()
        .map_err(|_| StoreError::Refused)?;
    if canonical.as_slice() != selection.candidate.as_slice()
        || canonical
            != proof
                .challenge
                .canonical_bytes()
                .map_err(|_| StoreError::Refused)?
        || expected.candidate.organization_id != expected.organization.as_str()
        || expected.candidate.principal_id != expected.principal.as_str()
        || expected.candidate.base_revision != expected.checkpoint.revision
        || endorsement.device != expected.previous_device
        || endorsement.credential != expected.previous_credential
        || expected.candidate.device_id == expected.previous_device.as_str()
        || expected.candidate.credential_id == expected.previous_credential.as_str()
        || decode_hex::<32>(&expected.candidate.public_key).map_err(|_| StoreError::Refused)?
            == expected.previous_public_key
        || expected.credential.is_empty()
        || expected.credential.len() > 1024
        || expected.rp_id.is_empty()
        || expected.rp_id.len() > 253
        || expected.origin.len() > 2048
        || expected.origin.contains('@')
    {
        return Err(StoreError::Refused);
    }
    let origin = Url::parse(&expected.origin).map_err(|_| StoreError::Refused)?;
    if origin.scheme() != "https"
        || origin.host_str() != Some(expected.rp_id.as_str())
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin.origin().ascii_serialization() != expected.origin
    {
        return Err(StoreError::Refused);
    }
    let message = existing_client_message(
        &expected.organization,
        &expected.principal,
        &expected.previous_device,
        &expected.previous_credential,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
    )?;
    let key =
        VerifyingKey::from_bytes(&expected.previous_public_key).map_err(|_| StoreError::Refused)?;
    if key.is_weak()
        || key
            .verify_strict(&message, &Signature::from_bytes(&endorsement.signature))
            .is_err()
    {
        return Err(StoreError::Refused);
    }
    let context = additional_context_digest(
        &expected.organization,
        &expected.principal,
        expected.checkpoint,
        expected.policy,
        &expected.candidate,
        endorsement,
    )?;
    let digest = additional_commitment(
        &expected.candidate,
        &expected.organization,
        &expected.principal,
        &expected.credential,
        &expected.rp_id,
        &expected.origin,
        &request.context,
        context,
    )
    .map_err(|_| StoreError::Refused)?;
    let options = &request.options;
    if request.context.issued_at > now
        || request.context.expires_at <= now
        || options.challenge != Base64UrlUnpadded::encode_string(&digest)
        || options.rp_id != expected.rp_id
        || options.user_verification != UserVerificationRequirement::Required
        || options.extensions.is_some()
        || !options.hints.is_empty()
        || options.timeout == 0
        || options.timeout > 300_000
        || options.allow_credentials.len() != 1
    {
        return Err(StoreError::Refused);
    }
    let allowed = &options.allow_credentials[0];
    if allowed.type_ != "public-key"
        || allowed.id != Base64UrlUnpadded::encode_string(&expected.credential)
        || !allowed.transports.is_empty()
    {
        return Err(StoreError::Refused);
    }
    Ok(CheckedAdditionalRequest {
        candidate: expected.candidate.clone(),
        options: request.options,
        checked_at: now,
    })
}

#[cfg(test)]
#[path = "client/tests.rs"]
mod tests;
