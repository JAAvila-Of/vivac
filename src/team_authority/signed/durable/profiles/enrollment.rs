use super::super::pending::coordinator::additional::ExistingClientEndorsement;
use super::super::pending::CandidateProposal;
use super::*;
use crate::approval::durable::{OwnedAdditionalChallenge, PublicPasskeyState};
use crate::approval::{AssertionRequest, EnrollmentAssertionVerifier};
use crate::enrollment_proof::{Challenge, Proof};
use crate::team_authority::signed::v2::registry::admission::{
    AdmissionAuthority, AdmissionRecord, AdmissionRequest, FixturePasskeyAssociation,
    MAX_ADMISSION_BYTES,
};
use crate::team_authority::signed::v2::registry::SignedRevocation;
use crate::team_authority::{CredentialId, DeviceId, OperationId};

#[derive(Clone)]
pub(crate) struct FixtureEnrollmentTrust {
    pub(crate) registered: FixtureRegisteredTrust,
    pub(crate) rp_id: String,
    pub(crate) origin: String,
    pub(crate) associations: Vec<FixturePasskeyAssociation>,
}

impl FixtureEnrollmentTrust {
    pub(crate) fn bootstrap_checkpoint(&self) -> StoreResult<Checkpoint> {
        if self.associations.is_empty() || self.associations.len() > 128 {
            return Err(StoreError::Refused);
        }
        let registered = self.registered.bootstrap_checkpoint()?;
        let verifier = self.verifier()?;
        self.authority()?;
        let mut associations: Vec<_> = self
            .associations
            .iter()
            .map(|association| {
                let mut bytes = Vec::with_capacity(84);
                bytes.extend_from_slice(association.device.as_str().as_bytes());
                bytes.extend_from_slice(association.credential.as_str().as_bytes());
                bytes.extend_from_slice(&association.state.commitment());
                bytes
            })
            .collect();
        associations.sort_unstable();
        let mut digest = Sha256::new();
        digest.update(b"vivac.team.authority.enrollment-fixture.v1\0");
        digest.update(registered.digest);
        digest.update(verifier.configuration_commitment());
        digest.update((associations.len() as u16).to_be_bytes());
        for association in associations {
            digest.update(association);
        }
        Ok(Checkpoint {
            revision: 0,
            digest: digest.finalize().into(),
        })
    }
    fn verifier(&self) -> StoreResult<EnrollmentAssertionVerifier> {
        EnrollmentAssertionVerifier::new(&self.rp_id, &self.origin).map_err(assertion_error)
    }
    fn authority(&self) -> StoreResult<AdmissionAuthority> {
        if self.associations.is_empty() || self.associations.len() > 128 {
            return Err(StoreError::Refused);
        }
        let base = &self.registered.base;
        AdmissionAuthority::bootstrap_fixture(
            base.organization.clone(),
            base.initial_principal.clone(),
            self.registered.policy_admin.clone(),
            base.known.clone(),
            &self.associations,
        )
        .map_err(|_| StoreError::Refused)
    }
}

struct LiveAdmission {
    checkpoint: Checkpoint,
    request: [u8; 32],
    deadline: std::time::Instant,
}

pub(crate) struct EnrollmentRequest {
    kind: AdmissionRequest,
    encoded: Vec<u8>,
    live: Option<LiveAdmission>,
}

impl EnrollmentRequest {
    pub(crate) fn policy(signed: SignedRequestV2) -> Self {
        let encoded = signed.request.canonical_bytes();
        Self {
            kind: AdmissionRequest::Policy(signed),
            encoded,
            live: None,
        }
    }
    pub(crate) fn revoke(signed: SignedRevocation) -> Self {
        let encoded = signed.request.canonical_bytes();
        Self {
            kind: AdmissionRequest::Revoke(signed),
            encoded,
            live: None,
        }
    }
    pub(crate) fn canonical_bytes(&self) -> StoreResult<Vec<u8>> {
        Ok(self.encoded.clone())
    }
    fn live_record(
        record: AdmissionRecord,
        checkpoint: Checkpoint,
        deadline: std::time::Instant,
    ) -> StoreResult<Self> {
        let encoded = record.canonical_bytes().map_err(|_| StoreError::Refused)?;
        let request = Sha256::digest(&encoded).into();
        Ok(Self {
            kind: AdmissionRequest::Enroll(Box::new(record)),
            encoded,
            live: Some(LiveAdmission {
                checkpoint,
                request,
                deadline,
            }),
        })
    }
}

impl Profile for EnrollmentProfile {
    type Trust = FixtureEnrollmentTrust;
    type Authority = AdmissionAuthority;
    type SignedRequest = EnrollmentRequest;
    const VERSION: u64 = 5;
    const METADATA_SCHEMA: &'static str = "CREATE TABLE meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL CHECK(version=5), organization BLOB NOT NULL CHECK(length(organization)=26), initial_principal BLOB NOT NULL CHECK(length(initial_principal)=26), trust BLOB NOT NULL CHECK(length(trust)=32), revision INTEGER NOT NULL CHECK(revision BETWEEN 0 AND 1024), head BLOB NOT NULL CHECK(length(head)=32)) STRICT";
    const INSERT_METADATA: &'static str = "INSERT INTO meta VALUES(1,5,?1,?2,?3,0,?3)";
    const ENTRY_SCHEMA: &'static str = "CREATE TABLE entries (seq INTEGER PRIMARY KEY CHECK(seq BETWEEN 1 AND 1024), operation BLOB NOT NULL UNIQUE CHECK(length(operation)=26), request BLOB NOT NULL CHECK(length(request) BETWEEN 186 AND 213 OR length(request)=226 OR length(request) BETWEEN 527 AND 16910), signature BLOB NOT NULL CHECK(length(signature)=64)) STRICT";
    const MAX_ENVELOPE_BYTES: usize = MAX_ADMISSION_BYTES;
    fn base(trust: &Self::Trust) -> &FixtureTrust {
        &trust.registered.base
    }
    fn bootstrap_checkpoint(trust: &Self::Trust) -> StoreResult<Checkpoint> {
        trust.bootstrap_checkpoint()
    }
    fn bootstrap(trust: &Self::Trust) -> StoreResult<Self::Authority> {
        trust.authority()
    }
    fn device<'a>(
        _trust: &'a Self::Trust,
        _request: &Self::SignedRequest,
    ) -> StoreResult<&'a KnownDevice> {
        Err(StoreError::Refused)
    }
    fn apply(
        _trust: &Self::Trust,
        _authority: &mut Self::Authority,
        _request: &Self::SignedRequest,
    ) -> StoreResult<Outcome> {
        Err(StoreError::Refused)
    }
    fn apply_at(
        trust: &Self::Trust,
        authority: &mut Self::Authority,
        request: &Self::SignedRequest,
        checkpoint: Checkpoint,
    ) -> StoreResult<Outcome> {
        if request
            .kind
            .canonical_bytes()
            .map_err(|_| StoreError::Refused)?
            != request.encoded
        {
            return Err(StoreError::Refused);
        }
        authority
            .apply_at(&request.kind, checkpoint, &trust.verifier()?)
            .map_err(|_| StoreError::Refused)
    }
    fn apply_live(
        trust: &Self::Trust,
        authority: &mut Self::Authority,
        request: &Self::SignedRequest,
        checkpoint: Checkpoint,
    ) -> StoreResult<Outcome> {
        if let AdmissionRequest::Enroll(record) = &request.kind {
            let operation = OperationId::parse(&record.proof.challenge.operation_id)
                .map_err(|_| StoreError::Refused)?;
            if !authority.has_enrollment(&operation) {
                let live = request.live.as_ref().ok_or(StoreError::Refused)?;
                let digest: [u8; 32] = Sha256::digest(request.canonical_bytes()?).into();
                if live.checkpoint != checkpoint
                    || live.request != digest
                    || std::time::Instant::now() >= live.deadline
                {
                    return Err(StoreError::Refused);
                }
                valid_time(record, system_time()?)?;
            }
        }
        Self::apply_at(trust, authority, request, checkpoint)
    }
    fn parse_signed(bytes: &[u8], signature: [u8; 64]) -> StoreResult<Self::SignedRequest> {
        Ok(EnrollmentRequest {
            kind: AdmissionRequest::parse_canonical(bytes, signature)
                .map_err(|_| StoreError::Refused)?,
            encoded: bytes.to_vec(),
            live: None,
        })
    }
    fn canonical_bytes(request: &Self::SignedRequest) -> Vec<u8> {
        request.encoded.clone()
    }
    fn signature(request: &Self::SignedRequest) -> &[u8; 64] {
        request.kind.signature()
    }
    fn operation(request: &Self::SignedRequest) -> &[u8] {
        request.kind.operation().as_bytes()
    }
    fn revision(authority: &Self::Authority) -> u64 {
        authority.revision()
    }
    fn allows(
        authority: &Self::Authority,
        organization: &OrganizationId,
        principal: &PrincipalId,
        project: &ProjectId,
        permission: ProjectPermission,
    ) -> bool {
        authority.allows(organization, principal, project, permission)
    }
    fn schema() -> &'static [SchemaEntry] {
        const SCHEMA: &[SchemaEntry] = &[
            (
                b"table",
                b"entries",
                b"entries",
                Some(EnrollmentProfile::ENTRY_SCHEMA.as_bytes()),
            ),
            (
                b"table",
                b"meta",
                b"meta",
                Some(EnrollmentProfile::METADATA_SCHEMA.as_bytes()),
            ),
            (b"index", b"sqlite_autoindex_entries_1", b"entries", None),
        ];
        SCHEMA
    }
    fn create_auxiliary(_store: &ProfileStore<Self>) -> StoreResult<()> {
        Ok(())
    }
    fn validate_auxiliary(_store: &ProfileStore<Self>, _revision: u64) -> StoreResult<()> {
        Ok(())
    }
}

pub(crate) struct IssuedEnrollment {
    root: Identity,
    database: Identity,
    checkpoint: Checkpoint,
    candidate: Vec<u8>,
    deadline: std::time::Instant,
}
pub(crate) struct PendingEnrollment {
    issued: IssuedEnrollment,
    proof: Proof,
    endorsement: ExistingClientEndorsement,
    challenge: OwnedAdditionalChallenge,
}

impl AuthorityStoreEnrollment {
    pub(crate) fn reconcile_additional_enrollment(
        &mut self,
        proof: &Proof,
        endorsement: &ExistingClientEndorsement,
    ) -> StoreResult<Outcome> {
        let (authority, _) = self.snapshot()?;
        authority
            .reconcile_additional_enrollment(proof, endorsement)
            .map_err(|_| StoreError::Refused)
    }
    pub(crate) fn issue_additional_enrollment(
        &mut self,
        proposal: &CandidateProposal,
    ) -> StoreResult<(Challenge, IssuedEnrollment)> {
        self.enrollment_transaction(
            |store| {
                let (authority, checkpoint) = store.replay()?;
                store.expected = checkpoint;
                authority
                    .validate_additional_candidate(
                        &store.trust.registered.base.organization,
                        &proposal.principal,
                        &proposal.device,
                        &proposal.credential,
                        proposal.public_key,
                    )
                    .map_err(|_| StoreError::Refused)?;
                if !(1..=300).contains(&proposal.lifetime_secs) {
                    return Err(StoreError::Refused);
                }
                let issued_instant = std::time::Instant::now();
                let now = system_time()?;
                let expires_at = now
                    .checked_add(proposal.lifetime_secs)
                    .ok_or(StoreError::Refused)?;
                let deadline = issued_instant
                    .checked_add(std::time::Duration::from_secs(proposal.lifetime_secs))
                    .ok_or(StoreError::Refused)?;
                let mut random = [0; 58];
                getrandom::getrandom(&mut random).map_err(|_| StoreError::Unavailable)?;
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
                    organization_id: store.trust.registered.base.organization.as_str().into(),
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
                let issued = IssuedEnrollment {
                    root: store.root_identity,
                    database: store.database_identity,
                    checkpoint,
                    candidate: challenge
                        .canonical_bytes()
                        .map_err(|_| StoreError::Refused)?,
                    deadline,
                };
                Ok((challenge, issued))
            },
            |_| Ok(()),
        )
    }
    pub(crate) fn begin_additional_enrollment(
        &mut self,
        issued: IssuedEnrollment,
        proof: &Proof,
        endorsement: &ExistingClientEndorsement,
    ) -> StoreResult<(AssertionRequest, PendingEnrollment)> {
        self.enrollment_transaction(
            move |store| {
                let (authority, checkpoint) = store.replay()?;
                store.expected = checkpoint;
                check_issued(store, &issued, checkpoint, proof)?;
                let verifier = store.trust.verifier()?;
                let (context, state) = authority
                    .check_candidate_context(proof, endorsement, checkpoint)
                    .map_err(|_| StoreError::Refused)?;
                let (request, challenge) = verifier
                    .start_recorded_additional(proof, &state, context)
                    .map_err(assertion_error)?;
                Ok((
                    request,
                    PendingEnrollment {
                        issued,
                        proof: proof.clone(),
                        endorsement: endorsement.clone(),
                        challenge,
                    },
                ))
            },
            |_| Ok(()),
        )
    }
    pub(crate) fn finish_additional_enrollment(
        &mut self,
        pending: PendingEnrollment,
        assertion: &[u8],
    ) -> StoreResult<Outcome> {
        let (outcome, admitted, _request) = self.enrollment_transaction(
            move |store| {
                let (authority, checkpoint) = store.replay()?;
                store.expected = checkpoint;
                check_issued(store, &pending.issued, checkpoint, &pending.proof)?;
                let verifier = store.trust.verifier()?;
                let (context, state) = authority
                    .check_candidate_context(&pending.proof, &pending.endorsement, checkpoint)
                    .map_err(|_| StoreError::Refused)?;
                let now = system_time()?;
                let (witness, _) = verifier
                    .finish_recorded_additional(
                        pending.challenge,
                        &pending.proof,
                        &state,
                        assertion,
                        context,
                        now,
                    )
                    .map_err(assertion_error)?;
                let record = AdmissionRecord {
                    proof: pending.proof,
                    endorsement: pending.endorsement,
                    witness,
                    accepted_at: now,
                };
                let request =
                    EnrollmentRequest::live_record(record, checkpoint, pending.issued.deadline)?;
                let (outcome, admitted) = store.admit_in_transaction(&request)?;
                Ok((outcome, admitted, request))
            },
            |(_, _, request)| {
                if let AdmissionRequest::Enroll(record) = &request.kind {
                    valid_time(record, system_time()?)?;
                }
                if std::time::Instant::now()
                    >= request.live.as_ref().ok_or(StoreError::Refused)?.deadline
                {
                    return Err(StoreError::Refused);
                }
                Ok(())
            },
        )?;
        self.expected = admitted;
        Ok(outcome)
    }
    fn enrollment_transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> StoreResult<T>,
        finalizer: impl FnOnce(&T) -> StoreResult<()>,
    ) -> StoreResult<T> {
        self.validate_files()?;
        self.database
            .execute("BEGIN IMMEDIATE")
            .map_err(|_| StoreError::Unpublished)?;
        let mut transaction = Transaction::new(&self.database);
        let result = operation(self).and_then(|value| {
            self.validate_files()?;
            Ok(value)
        });
        match result {
            Ok(value) => {
                if self.fault(FaultPoint::BeforeCommit) {
                    self.rollback()?;
                    return Err(StoreError::Unpublished);
                }
                if let Err(error) = finalizer(&value) {
                    self.rollback()?;
                    return Err(error);
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
                Ok(value)
            }
            Err(error) => {
                self.rollback()?;
                Err(error)
            }
        }
    }
    pub(crate) fn binding_is_active(
        &mut self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        device: &DeviceId,
        credential: &CredentialId,
    ) -> StoreResult<bool> {
        let (authority, _) = self.snapshot()?;
        Ok(authority
            .known_device(organization, principal, device, credential)
            .is_ok())
    }
    pub(crate) fn passkey_state(
        &mut self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        device: &DeviceId,
        credential: &CredentialId,
    ) -> StoreResult<PublicPasskeyState> {
        let (authority, _) = self.snapshot()?;
        authority
            .passkey_state(organization, principal, device, credential)
            .cloned()
            .map_err(|_| StoreError::Refused)
    }
    pub(crate) fn enrollment_nonce_consumed(&mut self, nonce: [u8; 32]) -> StoreResult<bool> {
        let (authority, _) = self.snapshot()?;
        Ok(authority.nonce_is_consumed(nonce))
    }
}

fn check_issued(
    store: &AuthorityStoreEnrollment,
    issued: &IssuedEnrollment,
    checkpoint: Checkpoint,
    proof: &Proof,
) -> StoreResult<()> {
    if issued.root != store.root_identity
        || issued.database != store.database_identity
        || issued.checkpoint != checkpoint
        || std::time::Instant::now() >= issued.deadline
        || proof.challenge.base_revision != checkpoint.revision
        || proof
            .challenge
            .canonical_bytes()
            .map_err(|_| StoreError::Refused)?
            != issued.candidate
    {
        return Err(StoreError::Refused);
    }
    proof
        .verify(system_time()?)
        .map_err(|_| StoreError::Refused)
}
fn valid_time(record: &AdmissionRecord, now: u64) -> StoreResult<()> {
    record.proof.verify(now).map_err(|_| StoreError::Refused)?;
    let context = record.witness.context();
    if now < context.issued_at || now >= context.expires_at || record.accepted_at > now {
        return Err(StoreError::Refused);
    }
    Ok(())
}
fn assertion_error(error: crate::approval::AssertionError) -> StoreError {
    match error {
        crate::approval::AssertionError::Refused => StoreError::Refused,
        crate::approval::AssertionError::Unavailable => StoreError::Unavailable,
    }
}
fn system_time() -> StoreResult<u64> {
    crate::enrollment_proof::current_time().map_err(|_| StoreError::Unavailable)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[path = "enrollment/tests.rs"]
mod tests;
