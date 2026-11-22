use super::{sql, AuthorityStore, Checkpoint, FaultPoint, StoreError, StoreResult, Transaction};
use crate::enrollment_proof::{canonical_identifier, decode_hex, Challenge, Proof};
use crate::team_authority::{CredentialId, DeviceId, OperationId, PrincipalId};

pub(super) const SCHEMA: &str = "CREATE TABLE pending (request BLOB PRIMARY KEY NOT NULL CHECK(length(request)=26), operation BLOB NOT NULL UNIQUE CHECK(length(operation)=26), nonce BLOB NOT NULL UNIQUE CHECK(length(nonce)=32), challenge BLOB NOT NULL CHECK(length(challenge) BETWEEN 1 AND 8192), proof BLOB CHECK(proof IS NULL OR length(proof) BETWEEN 1 AND 8192), observed_at INTEGER CHECK(observed_at IS NULL OR observed_at>=0), CHECK((proof IS NULL)=(observed_at IS NULL))) STRICT";
const MAX_PENDING: usize = 128;
const MAX_JSON: usize = 8192;

#[derive(Clone, Debug)]
pub(crate) struct CandidateProposal {
    pub principal: PrincipalId,
    pub device: DeviceId,
    pub credential: CredentialId,
    pub public_key: [u8; 32],
    pub lifetime_secs: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Observation {
    pub challenge: Challenge,
    pub proof: Option<Proof>,
    pub observed_at: Option<u64>,
}

struct Record {
    request: String,
    observation: Observation,
}

impl AuthorityStore {
    pub(crate) fn issue_candidate(
        &mut self,
        request: &OperationId,
        proposal: &CandidateProposal,
    ) -> StoreResult<Challenge> {
        self.issue_candidate_using(request, proposal, system_time, system_entropy)
    }

    pub(crate) fn observe_candidate(&mut self, proof: &[u8]) -> StoreResult<Observation> {
        self.observe_candidate_using(proof, system_time)
    }

    // This is historical pending evidence, never enrollment or approval status.
    pub(crate) fn candidate(&mut self, operation: &OperationId) -> StoreResult<Observation> {
        self.validate_files()?;
        self.database.execute("BEGIN")?;
        let mut transaction = Transaction::new(&self.database);
        let result = (|| {
            let (_, checkpoint) = self.replay()?;
            self.expected = checkpoint;
            let record = records(self, checkpoint.revision)?
                .into_iter()
                .find(|record| record.observation.challenge.operation_id == operation.as_str())
                .ok_or(StoreError::Refused)?;
            self.validate_files()?;
            Ok(record.observation)
        })();
        match result {
            Ok(observation) => {
                transaction.disarm();
                if self.database.execute("COMMIT").is_err() {
                    self.poisoned = true;
                    return Err(StoreError::Unavailable);
                }
                self.validate_files()?;
                Ok(observation)
            }
            Err(error) => {
                self.rollback()?;
                Err(error)
            }
        }
    }

    fn issue_candidate_using(
        &mut self,
        request: &OperationId,
        proposal: &CandidateProposal,
        clock: fn() -> StoreResult<u64>,
        entropy: fn(&mut [u8]) -> StoreResult<()>,
    ) -> StoreResult<Challenge> {
        self.pending_transaction(|store, checkpoint| {
            let now = clock()?;
            let pending = records(store, checkpoint.revision)?;
            if let Some(record) = pending
                .iter()
                .find(|record| record.request == request.as_str())
            {
                let challenge = &record.observation.challenge;
                if challenge.principal_id != proposal.principal.as_str()
                    || challenge.device_id != proposal.device.as_str()
                    || challenge.credential_id != proposal.credential.as_str()
                    || decode_hex::<32>(&challenge.public_key).map_err(|_| StoreError::Refused)?
                        != proposal.public_key
                    || challenge.expires_at - challenge.issued_at != proposal.lifetime_secs
                    || challenge.base_revision != checkpoint.revision
                {
                    return Err(StoreError::Refused);
                }
                challenge.validate(now).map_err(|_| StoreError::Refused)?;
                return Ok(challenge.clone());
            }
            if pending.len() == MAX_PENDING || !(1..=300).contains(&proposal.lifetime_secs) {
                return Err(StoreError::Refused);
            }
            let expires_at = now
                .checked_add(proposal.lifetime_secs)
                .ok_or(StoreError::Refused)?;
            i64::try_from(expires_at).map_err(|_| StoreError::Refused)?;
            let mut challenge = Challenge {
                version: 1,
                organization_id: store.trust.organization.as_str().into(),
                principal_id: proposal.principal.as_str().into(),
                device_id: proposal.device.as_str().into(),
                credential_id: proposal.credential.as_str().into(),
                public_key: hex(&proposal.public_key),
                operation_id: request.as_str().into(),
                base_revision: checkpoint.revision,
                nonce: "00".repeat(32),
                issued_at: now,
                expires_at,
            };
            challenge.validate(now).map_err(|_| StoreError::Refused)?;
            let mut random = [0; 58];
            entropy(&mut random)?;
            let alphabet = b"0123456789abcdefghjkmnpqrstvwxyz";
            challenge.operation_id = random[..26]
                .iter()
                .enumerate()
                .map(|(index, byte)| {
                    char::from(alphabet[(byte & if index == 0 { 7 } else { 31 }) as usize])
                })
                .collect();
            challenge.nonce = hex(&random[26..]);
            let mut insert = store
                .database
                .prepare("INSERT INTO pending VALUES(?1,?2,?3,?4,NULL,NULL)")?;
            insert.bind_blob(1, request.as_str().as_bytes())?;
            insert.bind_blob(2, challenge.operation_id.as_bytes())?;
            insert.bind_blob(3, &random[26..])?;
            insert.bind_blob(4, &canonical_json(&challenge)?)?;
            insert.done().map_err(|_| StoreError::Unpublished)?;
            drop(insert);
            if store.fault(FaultPoint::AfterInsert) {
                return Err(StoreError::Unpublished);
            }
            Ok(challenge)
        })
    }

    fn observe_candidate_using(
        &mut self,
        bytes: &[u8],
        clock: fn() -> StoreResult<u64>,
    ) -> StoreResult<Observation> {
        if bytes.is_empty() || bytes.len() > MAX_JSON {
            return Err(StoreError::Refused);
        }
        let proof: Proof = serde_json::from_slice(bytes).map_err(|_| StoreError::Refused)?;
        self.pending_transaction(|store, checkpoint| {
            let now = clock()?;
            let record = records(store, checkpoint.revision)?
                .into_iter()
                .find(|record| {
                    record.observation.challenge.operation_id == proof.challenge.operation_id
                })
                .ok_or(StoreError::Refused)?;
            if proof.challenge.base_revision != checkpoint.revision
                || proof
                    .challenge
                    .canonical_bytes()
                    .map_err(|_| StoreError::Refused)?
                    != record
                        .observation
                        .challenge
                        .canonical_bytes()
                        .map_err(|_| StoreError::Refused)?
            {
                return Err(StoreError::Refused);
            }
            proof.verify(now).map_err(|_| StoreError::Refused)?;
            let canonical = canonical_json(&proof)?;
            if let Some(previous) = &record.observation.proof {
                if canonical_json(previous)? != canonical {
                    return Err(StoreError::Refused);
                }
                return Ok(record.observation);
            }
            let mut update = store.database.prepare(
                "UPDATE pending SET proof=?1,observed_at=?2 WHERE operation=?3 AND proof IS NULL",
            )?;
            update.bind_blob(1, &canonical)?;
            update.bind_integer(2, now)?;
            update.bind_blob(3, proof.challenge.operation_id.as_bytes())?;
            update.done().map_err(|_| StoreError::Unpublished)?;
            drop(update);
            if unsafe { sql::sqlite3_changes(store.database.0) } != 1 {
                return Err(StoreError::Unpublished);
            }
            if store.fault(FaultPoint::AfterInsert) {
                return Err(StoreError::Unpublished);
            }
            Ok(Observation {
                challenge: record.observation.challenge,
                proof: Some(proof),
                observed_at: Some(now),
            })
        })
    }

    fn pending_transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self, Checkpoint) -> StoreResult<T>,
    ) -> StoreResult<T> {
        self.validate_files()?;
        self.database
            .execute("BEGIN IMMEDIATE")
            .map_err(|_| StoreError::Unpublished)?;
        let mut transaction = Transaction::new(&self.database);
        let result = (|| {
            let (_, checkpoint) = self.replay()?;
            self.expected = checkpoint;
            let result = operation(self, checkpoint)?;
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

fn system_time() -> StoreResult<u64> {
    crate::enrollment_proof::current_time().map_err(|_| StoreError::Unavailable)
}

fn system_entropy(bytes: &mut [u8]) -> StoreResult<()> {
    getrandom::getrandom(bytes).map_err(|_| StoreError::Unavailable)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn validate(store: &AuthorityStore, revision: u64) -> StoreResult<()> {
    records(store, revision).map(|_| ())
}

fn records(store: &AuthorityStore, revision: u64) -> StoreResult<Vec<Record>> {
    let mut rows = store.database.prepare(
        "SELECT request,operation,nonce,challenge,proof,observed_at FROM pending ORDER BY request",
    )?;
    let mut records = Vec::new();
    while rows.row()? {
        if records.len() == MAX_PENDING {
            return Err(StoreError::Refused);
        }
        let request = String::from_utf8(rows.blob(0, 26)?).map_err(|_| StoreError::Refused)?;
        if !canonical_identifier(&request) {
            return Err(StoreError::Refused);
        }
        let challenge_bytes = rows.blob(3, MAX_JSON)?;
        let challenge: Challenge =
            serde_json::from_slice(&challenge_bytes).map_err(|_| StoreError::Refused)?;
        challenge
            .validate(challenge.issued_at)
            .map_err(|_| StoreError::Refused)?;
        if challenge.organization_id != store.trust.organization.as_str()
            || challenge.base_revision > revision
            || rows.blob(1, 26)? != challenge.operation_id.as_bytes()
            || rows.blob(2, 32)?
                != decode_hex::<32>(&challenge.nonce).map_err(|_| StoreError::Refused)?
            || canonical_json(&challenge)? != challenge_bytes
        {
            return Err(StoreError::Refused);
        }
        let proof_absent = unsafe { sql::sqlite3_column_type(rows.0, 4) } == sql::SQLITE_NULL;
        let time_absent = unsafe { sql::sqlite3_column_type(rows.0, 5) } == sql::SQLITE_NULL;
        if proof_absent != time_absent {
            return Err(StoreError::Refused);
        }
        let (proof, observed_at) = if proof_absent {
            (None, None)
        } else {
            let bytes = rows.blob(4, MAX_JSON)?;
            let proof: Proof = serde_json::from_slice(&bytes).map_err(|_| StoreError::Refused)?;
            let observed_at = rows.integer(5)?;
            proof.verify(observed_at).map_err(|_| StoreError::Refused)?;
            if canonical_json(&proof)? != bytes
                || proof
                    .challenge
                    .canonical_bytes()
                    .map_err(|_| StoreError::Refused)?
                    != challenge
                        .canonical_bytes()
                        .map_err(|_| StoreError::Refused)?
            {
                return Err(StoreError::Refused);
            }
            (Some(proof), Some(observed_at))
        };
        records.push(Record {
            request,
            observation: Observation {
                challenge,
                proof,
                observed_at,
            },
        });
    }
    Ok(records)
}

fn canonical_json(value: &impl serde::Serialize) -> StoreResult<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|_| StoreError::Refused)?;
    if bytes.is_empty() || bytes.len() > MAX_JSON {
        return Err(StoreError::Refused);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "pending/tests.rs"]
mod tests;

#[path = "pending/coordinator.rs"]
pub(crate) mod coordinator;
