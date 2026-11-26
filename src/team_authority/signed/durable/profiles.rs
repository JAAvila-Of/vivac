use super::*;
use crate::team_authority::signed::v2::{DeviceRequestV2, SignedAuthorityV2, SignedRequestV2};

#[cfg(test)]
#[path = "profiles/tests.rs"]
mod tests;

#[path = "profiles/registered.rs"]
mod registered;

#[cfg(feature = "coordinator-probe")]
#[path = "profiles/enrollment.rs"]
pub(crate) mod enrollment;

mod sealed {
    pub trait Sealed {}
}

pub(crate) struct V1Profile;
pub(crate) struct V2Profile;
pub(crate) struct RegisteredProfile;
#[cfg(feature = "coordinator-probe")]
pub(crate) struct EnrollmentProfile;

impl sealed::Sealed for V1Profile {}
impl sealed::Sealed for V2Profile {}
impl sealed::Sealed for RegisteredProfile {}
#[cfg(feature = "coordinator-probe")]
impl sealed::Sealed for EnrollmentProfile {}

pub(crate) type SchemaEntry = (
    &'static [u8],
    &'static [u8],
    &'static [u8],
    Option<&'static [u8]>,
);

pub(crate) trait Profile: sealed::Sealed + Sized {
    type Trust;
    type Authority;
    type SignedRequest;

    const VERSION: u64;
    const METADATA_SCHEMA: &'static str;
    const INSERT_METADATA: &'static str;
    const ENTRY_SCHEMA: &'static str;
    const MAX_ENVELOPE_BYTES: usize;

    fn base(trust: &Self::Trust) -> &FixtureTrust;
    fn bootstrap_checkpoint(trust: &Self::Trust) -> StoreResult<Checkpoint>;
    fn bootstrap(trust: &Self::Trust) -> StoreResult<Self::Authority>;
    fn device<'a>(
        trust: &'a Self::Trust,
        request: &Self::SignedRequest,
    ) -> StoreResult<&'a KnownDevice>;
    fn apply(
        trust: &Self::Trust,
        authority: &mut Self::Authority,
        request: &Self::SignedRequest,
    ) -> StoreResult<Outcome>;
    fn apply_at(
        trust: &Self::Trust,
        authority: &mut Self::Authority,
        request: &Self::SignedRequest,
        _checkpoint: Checkpoint,
    ) -> StoreResult<Outcome> {
        Self::apply(trust, authority, request)
    }
    fn apply_live(
        trust: &Self::Trust,
        authority: &mut Self::Authority,
        request: &Self::SignedRequest,
        checkpoint: Checkpoint,
    ) -> StoreResult<Outcome> {
        Self::apply_at(trust, authority, request, checkpoint)
    }
    fn parse_signed(bytes: &[u8], signature: [u8; 64]) -> StoreResult<Self::SignedRequest>;
    fn canonical_bytes(request: &Self::SignedRequest) -> Vec<u8>;
    fn signature(request: &Self::SignedRequest) -> &[u8; 64];
    fn operation(request: &Self::SignedRequest) -> &[u8];
    fn revision(authority: &Self::Authority) -> u64;
    fn allows(
        authority: &Self::Authority,
        organization: &OrganizationId,
        principal: &PrincipalId,
        project: &ProjectId,
        permission: ProjectPermission,
    ) -> bool;
    fn schema() -> &'static [SchemaEntry];
    fn create_auxiliary(store: &ProfileStore<Self>) -> StoreResult<()>;
    fn validate_auxiliary(store: &ProfileStore<Self>, revision: u64) -> StoreResult<()>;
}

impl Profile for V1Profile {
    type Trust = FixtureTrust;
    type Authority = SignedAuthority;
    type SignedRequest = SignedRequest;

    const VERSION: u64 = 2;
    const METADATA_SCHEMA: &'static str = super::METADATA_SCHEMA;
    const INSERT_METADATA: &'static str = "INSERT INTO meta VALUES(1,2,?1,?2,?3,0,?3)";
    const ENTRY_SCHEMA: &'static str = super::ENTRY_SCHEMA;
    const MAX_ENVELOPE_BYTES: usize = super::MAX_ENVELOPE_BYTES;

    fn base(trust: &FixtureTrust) -> &FixtureTrust {
        trust
    }

    fn bootstrap_checkpoint(trust: &FixtureTrust) -> StoreResult<Checkpoint> {
        trust.bootstrap_checkpoint()
    }

    fn bootstrap(trust: &FixtureTrust) -> StoreResult<SignedAuthority> {
        Ok(SignedAuthority::bootstrap(
            trust.organization.clone(),
            trust.initial_principal.clone(),
        ))
    }

    fn apply(
        trust: &FixtureTrust,
        authority: &mut SignedAuthority,
        request: &SignedRequest,
    ) -> StoreResult<Outcome> {
        let device = Self::device(trust, request)?;
        authority
            .apply_signed(request, device)
            .map_err(|_| StoreError::Refused)
    }

    fn device<'a>(
        trust: &'a FixtureTrust,
        request: &SignedRequest,
    ) -> StoreResult<&'a KnownDevice> {
        trust.device(&request.request)
    }

    fn parse_signed(bytes: &[u8], signature: [u8; 64]) -> StoreResult<SignedRequest> {
        Ok(SignedRequest {
            request: DeviceRequest::parse_canonical(bytes).map_err(|_| StoreError::Refused)?,
            signature,
        })
    }

    fn canonical_bytes(request: &SignedRequest) -> Vec<u8> {
        request.request.canonical_bytes()
    }

    fn signature(request: &SignedRequest) -> &[u8; 64] {
        &request.signature
    }

    fn operation(request: &SignedRequest) -> &[u8] {
        request.request.change.operation.as_str().as_bytes()
    }

    fn revision(authority: &SignedAuthority) -> u64 {
        authority.revision()
    }

    fn allows(
        authority: &SignedAuthority,
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
                Some(ENTRY_SCHEMA.as_bytes()),
            ),
            (
                b"table",
                b"meta",
                b"meta",
                Some(V1Profile::METADATA_SCHEMA.as_bytes()),
            ),
            (
                b"table",
                b"pending",
                b"pending",
                Some(pending::SCHEMA.as_bytes()),
            ),
            (b"index", b"sqlite_autoindex_entries_1", b"entries", None),
            (b"index", b"sqlite_autoindex_pending_1", b"pending", None),
            (b"index", b"sqlite_autoindex_pending_2", b"pending", None),
            (b"index", b"sqlite_autoindex_pending_3", b"pending", None),
        ];
        SCHEMA
    }

    fn create_auxiliary(store: &AuthorityStore) -> StoreResult<()> {
        store.database.execute(pending::SCHEMA)
    }

    fn validate_auxiliary(store: &AuthorityStore, revision: u64) -> StoreResult<()> {
        pending::validate(store, revision)
    }
}

impl Profile for V2Profile {
    type Trust = FixtureTrustV2;
    type Authority = SignedAuthorityV2;
    type SignedRequest = SignedRequestV2;

    const VERSION: u64 = 3;
    const METADATA_SCHEMA: &'static str = "CREATE TABLE meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL CHECK(version=3), organization BLOB NOT NULL CHECK(length(organization)=26), initial_principal BLOB NOT NULL CHECK(length(initial_principal)=26), trust BLOB NOT NULL CHECK(length(trust)=32), revision INTEGER NOT NULL CHECK(revision BETWEEN 0 AND 1024), head BLOB NOT NULL CHECK(length(head)=32)) STRICT";
    const INSERT_METADATA: &'static str = "INSERT INTO meta VALUES(1,3,?1,?2,?3,0,?3)";
    const ENTRY_SCHEMA: &'static str = super::ENTRY_SCHEMA;
    const MAX_ENVELOPE_BYTES: usize = super::MAX_ENVELOPE_BYTES;

    fn base(trust: &FixtureTrustV2) -> &FixtureTrust {
        &trust.base
    }

    fn bootstrap_checkpoint(trust: &FixtureTrustV2) -> StoreResult<Checkpoint> {
        trust.bootstrap_checkpoint()
    }

    fn bootstrap(trust: &FixtureTrustV2) -> StoreResult<SignedAuthorityV2> {
        SignedAuthorityV2::bootstrap(
            trust.base.organization.clone(),
            trust.base.initial_principal.clone(),
            trust.policy_admin.clone(),
        )
        .map_err(|_| StoreError::Refused)
    }

    fn apply(
        trust: &FixtureTrustV2,
        authority: &mut SignedAuthorityV2,
        request: &SignedRequestV2,
    ) -> StoreResult<Outcome> {
        let device = Self::device(trust, request)?;
        authority
            .apply_signed(request, device)
            .map_err(|_| StoreError::Refused)
    }

    fn device<'a>(
        trust: &'a FixtureTrustV2,
        signed: &SignedRequestV2,
    ) -> StoreResult<&'a KnownDevice> {
        let request = &signed.request;
        trust
            .base
            .known
            .iter()
            .find(|binding| {
                let (organization, principal, device, credential, _) = binding.binding_fields();
                organization == &request.organization
                    && principal == &request.actor
                    && device == &request.device
                    && credential == &request.credential
            })
            .ok_or(StoreError::Refused)
    }

    fn parse_signed(bytes: &[u8], signature: [u8; 64]) -> StoreResult<SignedRequestV2> {
        Ok(SignedRequestV2 {
            request: DeviceRequestV2::parse_canonical(bytes).map_err(|_| StoreError::Refused)?,
            signature,
        })
    }

    fn canonical_bytes(request: &SignedRequestV2) -> Vec<u8> {
        request.request.canonical_bytes()
    }

    fn signature(request: &SignedRequestV2) -> &[u8; 64] {
        &request.signature
    }

    fn operation(request: &SignedRequestV2) -> &[u8] {
        request.request.operation.as_str().as_bytes()
    }

    fn revision(authority: &SignedAuthorityV2) -> u64 {
        authority.revision()
    }

    fn allows(
        authority: &SignedAuthorityV2,
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
                Some(ENTRY_SCHEMA.as_bytes()),
            ),
            (
                b"table",
                b"meta",
                b"meta",
                Some(V2Profile::METADATA_SCHEMA.as_bytes()),
            ),
            (b"index", b"sqlite_autoindex_entries_1", b"entries", None),
        ];
        SCHEMA
    }

    fn create_auxiliary(_store: &AuthorityStoreV2) -> StoreResult<()> {
        Ok(())
    }

    fn validate_auxiliary(_store: &AuthorityStoreV2, _revision: u64) -> StoreResult<()> {
        Ok(())
    }
}

impl AuthorityStoreV2 {
    pub(crate) fn can_add_own_device(
        &mut self,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> StoreResult<bool> {
        let (authority, _) = self.snapshot()?;
        Ok(authority.can_add_own_device(organization, principal))
    }

    pub(crate) fn can_manage_enrollment_policy(
        &mut self,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> StoreResult<bool> {
        let (authority, _) = self.snapshot()?;
        Ok(authority.can_manage_enrollment_policy(organization, principal))
    }
}
