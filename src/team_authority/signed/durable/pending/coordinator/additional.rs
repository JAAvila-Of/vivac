use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use super::{
    AssertionRequest, AuthorityStore, Checkpoint, CoordinatedPending, EnrollmentAssertionVerifier,
    ScopedAssertion, StoreError, StoreResult, TrustedPasskeyBinding,
};
use crate::enrollment_proof::{decode_hex, Challenge, Proof};
use crate::team_authority::signed::SignedAuthority;
use crate::team_authority::{CredentialId, DeviceId, OperationId, OrganizationId, PrincipalId};
use ed25519_dalek::{Signature, VerifyingKey};

const MAX_GRANTS: usize = 128;
const POLICY_DOMAIN: &[u8] = b"vivac.fixture.additional-device-policy.v1\0";
const ENDORSEMENT_DOMAIN: &[u8] = b"vivac.device.additional-endorsement.v1\0";

// Unverified public input: only a registry-selected key can validate this signature.
#[derive(Clone)]
pub(crate) struct ExistingClientEndorsement {
    pub(crate) device: DeviceId,
    pub(crate) credential: CredentialId,
    pub(crate) signature: [u8; 64],
}

// This catalog is explicit fixture trust, not an admitted policy or client input.
pub(crate) struct FixtureAdditionalDevicePolicy {
    organization: OrganizationId,
    principals: BTreeSet<PrincipalId>,
}

impl FixtureAdditionalDevicePolicy {
    pub(crate) fn new(
        organization: OrganizationId,
        principals: Vec<PrincipalId>,
    ) -> StoreResult<Self> {
        if principals.len() > MAX_GRANTS {
            return Err(StoreError::Refused);
        }
        let count = principals.len();
        let principals: BTreeSet<_> = principals.into_iter().collect();
        if principals.len() != count {
            return Err(StoreError::Refused);
        }
        Ok(Self {
            organization,
            principals,
        })
    }

    fn fingerprint(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(POLICY_DOMAIN);
        digest.update(self.organization.as_str().as_bytes());
        digest.update((self.principals.len() as u16).to_be_bytes());
        for principal in &self.principals {
            digest.update(principal.as_str().as_bytes());
        }
        digest.finalize().into()
    }
}

// This token has no conversion from a member-administration check token.
pub(crate) struct AdditionalPending {
    coordinated: CoordinatedPending,
    policy: [u8; 32],
    endorsement: ExistingClientEndorsement,
}

// A checked fixture policy does not establish human consent or device admission.
pub(crate) struct AdditionalScopedAssertion {
    coordinated: ScopedAssertion,
}

impl AdditionalScopedAssertion {
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        self.coordinated.checkpoint()
    }
}

impl AuthorityStore {
    pub(crate) fn begin_additional_device_assertion(
        &mut self,
        verifier: &EnrollmentAssertionVerifier,
        binding: &TrustedPasskeyBinding,
        operation: &OperationId,
        policy: &FixtureAdditionalDevicePolicy,
        endorsement: &ExistingClientEndorsement,
    ) -> StoreResult<(AssertionRequest, AdditionalPending)> {
        self.assertion_transaction(|store, authority, checkpoint| {
            let proof =
                super::current_proof(store, checkpoint, operation, super::super::system_time()?)?;
            check_policy(store, authority, binding, &proof, policy)?;
            verify_existing_client(store, binding, &proof, checkpoint, policy, endorsement)?;
            let context_digest = additional_context_digest(
                binding.organization(),
                binding.principal(),
                checkpoint,
                policy.fingerprint(),
                &proof.challenge,
                endorsement,
            )?;
            let (request, assertion) = verifier
                .start_additional(&proof, binding, context_digest)
                .map_err(super::assertion_error)?;
            Ok((
                request,
                AdditionalPending {
                    coordinated: CoordinatedPending {
                        assertion,
                        root: store.root_identity,
                        database: store.database_identity,
                        checkpoint,
                        operation: operation.clone(),
                        proof: super::super::canonical_json(&proof)?,
                    },
                    policy: policy.fingerprint(),
                    endorsement: endorsement.clone(),
                },
            ))
        })
    }

    pub(crate) fn finish_additional_device_assertion(
        &mut self,
        verifier: &EnrollmentAssertionVerifier,
        pending: AdditionalPending,
        binding: &mut TrustedPasskeyBinding,
        assertion: &[u8],
        policy: &FixtureAdditionalDevicePolicy,
    ) -> StoreResult<AdditionalScopedAssertion> {
        let AdditionalPending {
            coordinated: pending,
            policy: issued_policy,
            endorsement,
        } = pending;
        let checkpoint = pending.checkpoint;
        let prepared = self.assertion_transaction(move |store, authority, current| {
            if issued_policy != policy.fingerprint()
                || pending.root != store.root_identity
                || pending.database != store.database_identity
                || pending.checkpoint != current
            {
                return Err(StoreError::Refused);
            }
            let proof = super::current_proof(
                store,
                current,
                &pending.operation,
                super::super::system_time()?,
            )?;
            check_policy(store, authority, binding, &proof, policy)?;
            if super::super::canonical_json(&proof)? != pending.proof {
                return Err(StoreError::Refused);
            }
            verify_existing_client(store, binding, &proof, current, policy, &endorsement)?;
            let context_digest = additional_context_digest(
                binding.organization(),
                binding.principal(),
                current,
                policy.fingerprint(),
                &proof.challenge,
                &endorsement,
            )?;
            verifier
                .prepare_additional(
                    pending.assertion,
                    &proof,
                    binding,
                    assertion,
                    context_digest,
                )
                .map_err(super::assertion_error)
        })?;
        Ok(AdditionalScopedAssertion {
            coordinated: ScopedAssertion {
                assertion: prepared.publish(),
                checkpoint,
            },
        })
    }
}

pub(crate) fn additional_context_digest(
    organization: &OrganizationId,
    principal: &PrincipalId,
    checkpoint: Checkpoint,
    policy: [u8; 32],
    candidate: &Challenge,
    endorsement: &ExistingClientEndorsement,
) -> StoreResult<[u8; 32]> {
    let message = existing_client_message(
        organization,
        principal,
        &endorsement.device,
        &endorsement.credential,
        checkpoint,
        policy,
        candidate,
    )?;
    let mut digest = Sha256::new();
    digest.update(b"vivac.device.additional-context.v1\0");
    digest.update(message);
    digest.update(endorsement.signature);
    Ok(digest.finalize().into())
}

pub(crate) fn existing_client_message(
    organization: &OrganizationId,
    principal: &PrincipalId,
    device: &DeviceId,
    credential: &CredentialId,
    checkpoint: Checkpoint,
    policy: [u8; 32],
    candidate: &Challenge,
) -> StoreResult<Vec<u8>> {
    if candidate.organization_id != organization.as_str()
        || candidate.principal_id != principal.as_str()
        || candidate.base_revision != checkpoint.revision
    {
        return Err(StoreError::Refused);
    }
    let candidate = candidate
        .canonical_bytes()
        .map_err(|_| StoreError::Refused)?;
    let mut message =
        Vec::with_capacity(ENDORSEMENT_DOMAIN.len() + 4 * 26 + 8 + 64 + candidate.len());
    message.extend_from_slice(ENDORSEMENT_DOMAIN);
    for identifier in [
        organization.as_str(),
        principal.as_str(),
        device.as_str(),
        credential.as_str(),
    ] {
        message.extend_from_slice(identifier.as_bytes());
    }
    message.extend_from_slice(&checkpoint.revision.to_be_bytes());
    message.extend_from_slice(&checkpoint.digest);
    message.extend_from_slice(&policy);
    message.extend_from_slice(&candidate);
    Ok(message)
}

fn verify_existing_client(
    store: &AuthorityStore,
    binding: &TrustedPasskeyBinding,
    proof: &Proof,
    checkpoint: Checkpoint,
    policy: &FixtureAdditionalDevicePolicy,
    endorsement: &ExistingClientEndorsement,
) -> StoreResult<()> {
    let known = store
        .trust
        .known
        .iter()
        .find(|known| {
            let (organization, principal, device, credential, _) = known.binding_fields();
            organization == binding.organization()
                && principal == binding.principal()
                && device == &endorsement.device
                && credential == &endorsement.credential
        })
        .ok_or(StoreError::Refused)?;
    let (_, _, _, _, public_key) = known.binding_fields();
    let key = VerifyingKey::from_bytes(&public_key).map_err(|_| StoreError::Refused)?;
    if key.is_weak() {
        return Err(StoreError::Refused);
    }
    let message = existing_client_message(
        binding.organization(),
        binding.principal(),
        &endorsement.device,
        &endorsement.credential,
        checkpoint,
        policy.fingerprint(),
        &proof.challenge,
    )?;
    key.verify_strict(&message, &Signature::from_bytes(&endorsement.signature))
        .map_err(|_| StoreError::Refused)
}

fn check_policy(
    store: &AuthorityStore,
    authority: &SignedAuthority,
    binding: &TrustedPasskeyBinding,
    proof: &Proof,
    policy: &FixtureAdditionalDevicePolicy,
) -> StoreResult<()> {
    if policy.organization != store.trust.organization
        || binding.organization() != &policy.organization
        || !authority.is_active_principal(binding.organization(), binding.principal())
        || proof.challenge.principal_id != binding.principal().as_str()
        || !policy.principals.contains(binding.principal())
    {
        return Err(StoreError::Refused);
    }
    let public_key =
        decode_hex::<32>(&proof.challenge.public_key).map_err(|_| StoreError::Refused)?;
    let mut known_owner = false;
    for known in &store.trust.known {
        let (organization, principal, device, credential, key) = known.binding_fields();
        if organization != &policy.organization {
            return Err(StoreError::Refused);
        }
        known_owner |= principal == binding.principal();
        if proof.challenge.device_id == device.as_str()
            || proof.challenge.credential_id == credential.as_str()
            || public_key == key
        {
            return Err(StoreError::Refused);
        }
    }
    if !known_owner {
        return Err(StoreError::Refused);
    }
    Ok(())
}

#[cfg(test)]
#[path = "additional/tests.rs"]
mod tests;

#[path = "additional/client.rs"]
pub(crate) mod client;
