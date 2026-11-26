use super::pending::coordinator::additional::{
    additional_context_digest, existing_client_message, ExistingClientEndorsement,
};
use super::pending::CandidateProposal;
use super::{
    AuthorityStoreRegistered, Checkpoint, FaultPoint, Identity, StoreError, StoreResult,
    Transaction,
};
use crate::approval::{
    AssertionError, AssertionEvidence, AssertionRequest, EnrollmentAssertionVerifier,
    PendingAssertion, TrustedPasskeyBinding,
};
use crate::enrollment_proof::{decode_hex, Challenge, Proof};
use crate::team_authority::signed::v2::registry::RegisteredAuthority;
use crate::team_authority::{CredentialId, DeviceId, OperationId, OrganizationId, PrincipalId};
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

// This association is external fixture trust, not client-supplied enrollment.
pub(crate) struct FixtureRegisteredPasskey {
    binding: TrustedPasskeyBinding,
    device: DeviceId,
    credential: CredentialId,
}

impl FixtureRegisteredPasskey {
    pub(crate) fn new(
        binding: TrustedPasskeyBinding,
        device: DeviceId,
        credential: CredentialId,
    ) -> Self {
        Self {
            binding,
            device,
            credential,
        }
    }
}

// Issuance is process-local and neither cloneable nor importable as a receipt.
pub(crate) struct RegisteredIssuedCandidate {
    root: Identity,
    database: Identity,
    checkpoint: Checkpoint,
    candidate: Vec<u8>,
}

pub(crate) struct RegisteredAdditionalPending {
    issued: RegisteredIssuedCandidate,
    proof: Proof,
    endorsement: ExistingClientEndorsement,
    assertion: PendingAssertion,
}

// This checked view is not durable approval or an admitted device binding.
pub(crate) struct RegisteredScopedAssertion {
    _assertion: AssertionEvidence,
    checkpoint: Checkpoint,
}

impl RegisteredScopedAssertion {
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        self.checkpoint
    }
}

pub(crate) fn registered_policy_commitment(
    organization: &OrganizationId,
    principal: &PrincipalId,
    checkpoint: Checkpoint,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"vivac.device.registered-additional-policy.v1\0");
    digest.update(organization.as_str().as_bytes());
    digest.update(principal.as_str().as_bytes());
    digest.update(checkpoint.revision.to_be_bytes());
    digest.update(checkpoint.digest);
    digest.finalize().into()
}

impl AuthorityStoreRegistered {
    pub(crate) fn issue_registered_candidate(
        &mut self,
        proposal: &CandidateProposal,
    ) -> StoreResult<(Challenge, RegisteredIssuedCandidate)> {
        self.issue_registered_candidate_using(proposal, system_time, system_entropy)
    }

    fn issue_registered_candidate_using(
        &mut self,
        proposal: &CandidateProposal,
        clock: fn() -> StoreResult<u64>,
        entropy: fn(&mut [u8]) -> StoreResult<()>,
    ) -> StoreResult<(Challenge, RegisteredIssuedCandidate)> {
        self.registered_assertion_transaction(|store, authority, checkpoint| {
            authority
                .validate_additional_candidate(
                    &store.trust.base.organization,
                    &proposal.principal,
                    &proposal.device,
                    &proposal.credential,
                    proposal.public_key,
                )
                .map_err(|_| StoreError::Refused)?;
            if !(1..=300).contains(&proposal.lifetime_secs) {
                return Err(StoreError::Refused);
            }
            let now = clock()?;
            let expires_at = now
                .checked_add(proposal.lifetime_secs)
                .ok_or(StoreError::Refused)?;
            let mut random = [0; 58];
            entropy(&mut random)?;
            let alphabet = b"0123456789abcdefghjkmnpqrstvwxyz";
            let operation: String = random[..26]
                .iter()
                .enumerate()
                .map(|(index, byte)| {
                    char::from(alphabet[(byte & if index == 0 { 7 } else { 31 }) as usize])
                })
                .collect();
            if authority.operation_is_known(
                &OperationId::parse(&operation).map_err(|_| StoreError::Refused)?,
            ) {
                return Err(StoreError::Refused);
            }
            let challenge = Challenge {
                version: 1,
                organization_id: store.trust.base.organization.as_str().into(),
                principal_id: proposal.principal.as_str().into(),
                device_id: proposal.device.as_str().into(),
                credential_id: proposal.credential.as_str().into(),
                public_key: hex(&proposal.public_key),
                operation_id: operation,
                base_revision: checkpoint.revision,
                nonce: hex(&random[26..]),
                issued_at: now,
                expires_at,
            };
            challenge.validate(now).map_err(|_| StoreError::Refused)?;
            let issued = RegisteredIssuedCandidate {
                root: store.root_identity,
                database: store.database_identity,
                checkpoint,
                candidate: challenge
                    .canonical_bytes()
                    .map_err(|_| StoreError::Refused)?,
            };
            Ok((challenge, issued))
        })
    }

    pub(crate) fn begin_registered_additional_assertion(
        &mut self,
        verifier: &EnrollmentAssertionVerifier,
        passkey: &FixtureRegisteredPasskey,
        issued: RegisteredIssuedCandidate,
        proof: &Proof,
        endorsement: &ExistingClientEndorsement,
    ) -> StoreResult<(AssertionRequest, RegisteredAdditionalPending)> {
        self.registered_assertion_transaction(move |store, authority, checkpoint| {
            let context = check_registered(
                store,
                authority,
                (checkpoint, system_time()?),
                passkey,
                &issued,
                proof,
                endorsement,
            )?;
            let (request, assertion) = verifier
                .start_additional(proof, &passkey.binding, context)
                .map_err(assertion_error)?;
            Ok((
                request,
                RegisteredAdditionalPending {
                    issued,
                    proof: proof.clone(),
                    endorsement: endorsement.clone(),
                    assertion,
                },
            ))
        })
    }

    pub(crate) fn finish_registered_additional_assertion(
        &mut self,
        verifier: &EnrollmentAssertionVerifier,
        pending: RegisteredAdditionalPending,
        passkey: &mut FixtureRegisteredPasskey,
        assertion: &[u8],
    ) -> StoreResult<RegisteredScopedAssertion> {
        let checkpoint = pending.issued.checkpoint;
        let prepared =
            self.registered_assertion_transaction(move |store, authority, current| {
                let context = check_registered(
                    store,
                    authority,
                    (current, system_time()?),
                    passkey,
                    &pending.issued,
                    &pending.proof,
                    &pending.endorsement,
                )?;
                verifier
                    .prepare_additional(
                        pending.assertion,
                        &pending.proof,
                        &mut passkey.binding,
                        assertion,
                        context,
                    )
                    .map_err(assertion_error)
            })?;
        Ok(RegisteredScopedAssertion {
            _assertion: prepared.publish(),
            checkpoint,
        })
    }

    fn registered_assertion_transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self, &RegisteredAuthority, Checkpoint) -> StoreResult<T>,
    ) -> StoreResult<T> {
        self.validate_files()?;
        self.database
            .execute("BEGIN IMMEDIATE")
            .map_err(|_| StoreError::Unpublished)?;
        let mut transaction = Transaction::new(&self.database);
        let result = (|| {
            let (authority, checkpoint) = self.replay()?;
            self.expected = checkpoint;
            let result = operation(self, &authority, checkpoint)?;
            self.validate_files()?;
            Ok(result)
        })();
        match result {
            Ok(result) => {
                if self.fault(FaultPoint::BeforeCommit) {
                    self.rollback()?;
                    return Err(StoreError::Unpublished);
                }
                transaction.disarm();
                if self.database.execute("COMMIT").is_err() {
                    self.poisoned = true;
                    return Err(StoreError::CommitUnknown);
                }
                if self.fault(FaultPoint::AfterCommit) || self.validate_files().is_err() {
                    self.poisoned = true;
                    return Err(StoreError::CommitUnknown);
                }
                Ok(result)
            }
            Err(error) => {
                self.rollback()?;
                Err(error)
            }
        }
    }
}

fn check_registered(
    store: &AuthorityStoreRegistered,
    authority: &RegisteredAuthority,
    view: (Checkpoint, u64),
    passkey: &FixtureRegisteredPasskey,
    issued: &RegisteredIssuedCandidate,
    proof: &Proof,
    endorsement: &ExistingClientEndorsement,
) -> StoreResult<[u8; 32]> {
    let (checkpoint, now) = view;
    if issued.root != store.root_identity
        || issued.database != store.database_identity
        || issued.checkpoint != checkpoint
        || passkey.binding.organization() != &store.trust.base.organization
        || proof.challenge.organization_id != passkey.binding.organization().as_str()
        || proof.challenge.principal_id != passkey.binding.principal().as_str()
        || proof.challenge.base_revision != checkpoint.revision
        || endorsement.device != passkey.device
        || endorsement.credential != passkey.credential
    {
        return Err(StoreError::Refused);
    }
    proof.verify(now).map_err(|_| StoreError::Refused)?;
    if proof
        .challenge
        .canonical_bytes()
        .map_err(|_| StoreError::Refused)?
        != issued.candidate
    {
        return Err(StoreError::Refused);
    }
    let device = DeviceId::parse(&proof.challenge.device_id).map_err(|_| StoreError::Refused)?;
    let credential =
        CredentialId::parse(&proof.challenge.credential_id).map_err(|_| StoreError::Refused)?;
    let public = decode_hex::<32>(&proof.challenge.public_key).map_err(|_| StoreError::Refused)?;
    authority
        .validate_additional_candidate(
            passkey.binding.organization(),
            passkey.binding.principal(),
            &device,
            &credential,
            public,
        )
        .map_err(|_| StoreError::Refused)?;
    let known = authority
        .known_device(
            passkey.binding.organization(),
            passkey.binding.principal(),
            &passkey.device,
            &passkey.credential,
        )
        .map_err(|_| StoreError::Refused)?;
    let (_, _, _, _, key) = known.binding_fields();
    let key = VerifyingKey::from_bytes(&key).map_err(|_| StoreError::Refused)?;
    let policy = registered_policy_commitment(
        passkey.binding.organization(),
        passkey.binding.principal(),
        checkpoint,
    );
    let message = existing_client_message(
        passkey.binding.organization(),
        passkey.binding.principal(),
        &endorsement.device,
        &endorsement.credential,
        checkpoint,
        policy,
        &proof.challenge,
    )?;
    key.verify_strict(&message, &Signature::from_bytes(&endorsement.signature))
        .map_err(|_| StoreError::Refused)?;
    additional_context_digest(
        passkey.binding.organization(),
        passkey.binding.principal(),
        checkpoint,
        policy,
        &proof.challenge,
        endorsement,
    )
}

fn assertion_error(error: AssertionError) -> StoreError {
    match error {
        AssertionError::Refused => StoreError::Refused,
        AssertionError::Unavailable => StoreError::Unavailable,
    }
}

fn system_time() -> StoreResult<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| StoreError::Unavailable)
}

fn system_entropy(bytes: &mut [u8]) -> StoreResult<()> {
    getrandom::getrandom(bytes).map_err(|_| StoreError::Unavailable)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[path = "registered_assertion/tests.rs"]
mod tests;
