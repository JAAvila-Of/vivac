use super::super::Identity;
use super::{AuthorityStore, Checkpoint, FaultPoint, StoreError, StoreResult, Transaction};
use crate::approval::{
    AssertionError, AssertionEvidence, AssertionRequest, EnrollmentAssertionVerifier,
    PendingAssertion, TrustedPasskeyBinding,
};
use crate::enrollment_proof::Proof;
use crate::team_authority::signed::SignedAuthority;
use crate::team_authority::OperationId;

// This is a server-owned check token, not enrollment or human-approval evidence.
pub(crate) struct CoordinatedPending {
    assertion: PendingAssertion,
    root: Identity,
    database: Identity,
    checkpoint: Checkpoint,
    operation: OperationId,
    proof: Vec<u8>,
}

// The checkpoint records a checked view, never a durable admitted transition.
pub(crate) struct ScopedAssertion {
    assertion: AssertionEvidence,
    checkpoint: Checkpoint,
}

impl ScopedAssertion {
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        self.checkpoint
    }
}

impl AuthorityStore {
    pub(crate) fn begin_member_assertion(
        &mut self,
        verifier: &EnrollmentAssertionVerifier,
        binding: &TrustedPasskeyBinding,
        operation: &OperationId,
    ) -> StoreResult<(AssertionRequest, CoordinatedPending)> {
        self.begin_member_assertion_using(verifier, binding, operation, super::system_time)
    }

    pub(crate) fn finish_member_assertion(
        &mut self,
        verifier: &EnrollmentAssertionVerifier,
        pending: CoordinatedPending,
        binding: &mut TrustedPasskeyBinding,
        assertion: &[u8],
    ) -> StoreResult<ScopedAssertion> {
        self.finish_member_assertion_using(
            verifier,
            pending,
            binding,
            assertion,
            super::system_time,
        )
    }

    fn begin_member_assertion_using(
        &mut self,
        verifier: &EnrollmentAssertionVerifier,
        binding: &TrustedPasskeyBinding,
        operation: &OperationId,
        clock: fn() -> StoreResult<u64>,
    ) -> StoreResult<(AssertionRequest, CoordinatedPending)> {
        self.assertion_transaction(|store, authority, checkpoint| {
            check_member(store, authority, binding)?;
            let proof = current_proof(store, checkpoint, operation, clock()?)?;
            let (request, assertion) = verifier.start(&proof, binding).map_err(assertion_error)?;
            Ok((
                request,
                CoordinatedPending {
                    assertion,
                    root: store.root_identity,
                    database: store.database_identity,
                    checkpoint,
                    operation: operation.clone(),
                    proof: super::canonical_json(&proof)?,
                },
            ))
        })
    }

    fn finish_member_assertion_using(
        &mut self,
        verifier: &EnrollmentAssertionVerifier,
        pending: CoordinatedPending,
        binding: &mut TrustedPasskeyBinding,
        assertion: &[u8],
        clock: fn() -> StoreResult<u64>,
    ) -> StoreResult<ScopedAssertion> {
        let checkpoint = pending.checkpoint;
        let prepared = self.assertion_transaction(move |store, authority, current| {
            check_member(store, authority, binding)?;
            if pending.root != store.root_identity
                || pending.database != store.database_identity
                || pending.checkpoint != current
            {
                return Err(StoreError::Refused);
            }
            let proof = current_proof(store, current, &pending.operation, clock()?)?;
            if super::canonical_json(&proof)? != pending.proof {
                return Err(StoreError::Refused);
            }
            verifier
                .prepare(pending.assertion, &proof, binding, assertion)
                .map_err(assertion_error)
        })?;
        Ok(ScopedAssertion {
            assertion: prepared.publish(),
            checkpoint,
        })
    }

    fn assertion_transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self, &SignedAuthority, Checkpoint) -> StoreResult<T>,
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

fn check_member(
    store: &AuthorityStore,
    authority: &SignedAuthority,
    binding: &TrustedPasskeyBinding,
) -> StoreResult<()> {
    if binding.organization() != &store.trust.organization
        || !authority.can_manage_members(binding.organization(), binding.principal())
    {
        return Err(StoreError::Refused);
    }
    Ok(())
}

fn current_proof(
    store: &AuthorityStore,
    checkpoint: Checkpoint,
    operation: &OperationId,
    now: u64,
) -> StoreResult<Proof> {
    let proof = super::records(store, checkpoint.revision)?
        .into_iter()
        .find(|record| record.observation.challenge.operation_id == operation.as_str())
        .and_then(|record| record.observation.proof)
        .ok_or(StoreError::Refused)?;
    if proof.challenge.base_revision != checkpoint.revision {
        return Err(StoreError::Refused);
    }
    proof.verify(now).map_err(|_| StoreError::Refused)?;
    Ok(proof)
}

fn assertion_error(error: AssertionError) -> StoreError {
    match error {
        AssertionError::Refused => StoreError::Refused,
        AssertionError::Unavailable => StoreError::Unavailable,
    }
}

#[path = "coordinator/additional.rs"]
pub(crate) mod additional;

#[cfg(test)]
#[path = "coordinator/tests.rs"]
mod tests;
