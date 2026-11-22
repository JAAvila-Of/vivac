use super::*;
use crate::team_authority::signed::v2::registry::{
    RegisteredAuthority, RevocationRequest, SignedRevocation, REVOCATION_BYTES,
};
use crate::team_authority::{CredentialId, DeviceId};

#[cfg(test)]
#[path = "registered/tests.rs"]
mod tests;

impl Profile for RegisteredProfile {
    type Trust = FixtureRegisteredTrust;
    type Authority = RegisteredAuthority;
    type SignedRequest = RegisteredRequest;

    const VERSION: u64 = 4;
    const METADATA_SCHEMA: &'static str = "CREATE TABLE meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL CHECK(version=4), organization BLOB NOT NULL CHECK(length(organization)=26), initial_principal BLOB NOT NULL CHECK(length(initial_principal)=26), trust BLOB NOT NULL CHECK(length(trust)=32), revision INTEGER NOT NULL CHECK(revision BETWEEN 0 AND 1024), head BLOB NOT NULL CHECK(length(head)=32)) STRICT";
    const INSERT_METADATA: &'static str = "INSERT INTO meta VALUES(1,4,?1,?2,?3,0,?3)";
    const ENTRY_SCHEMA: &'static str = "CREATE TABLE entries (seq INTEGER PRIMARY KEY CHECK(seq BETWEEN 1 AND 1024), operation BLOB NOT NULL UNIQUE CHECK(length(operation)=26), request BLOB NOT NULL CHECK(length(request) BETWEEN 186 AND 213 OR length(request)=226), signature BLOB NOT NULL CHECK(length(signature)=64)) STRICT";
    const MAX_ENVELOPE_BYTES: usize = REVOCATION_BYTES;

    fn base(trust: &FixtureRegisteredTrust) -> &FixtureTrust {
        &trust.base
    }

    fn bootstrap_checkpoint(trust: &FixtureRegisteredTrust) -> StoreResult<Checkpoint> {
        trust.bootstrap_checkpoint()
    }

    fn bootstrap(trust: &FixtureRegisteredTrust) -> StoreResult<RegisteredAuthority> {
        RegisteredAuthority::bootstrap_fixture(
            trust.base.organization.clone(),
            trust.base.initial_principal.clone(),
            trust.policy_admin.clone(),
            trust.base.known.clone(),
        )
        .map_err(|_| StoreError::Refused)
    }

    fn device<'a>(
        _trust: &'a FixtureRegisteredTrust,
        _request: &RegisteredRequest,
    ) -> StoreResult<&'a KnownDevice> {
        // Current validity cannot be resolved from the bootstrap catalogue alone.
        Err(StoreError::Refused)
    }

    fn apply(
        _trust: &FixtureRegisteredTrust,
        authority: &mut RegisteredAuthority,
        request: &RegisteredRequest,
    ) -> StoreResult<Outcome> {
        match request {
            RegisteredRequest::Policy(signed) => authority
                .apply_signed(signed)
                .map_err(|_| StoreError::Refused),
            RegisteredRequest::Revoke(signed) => authority
                .apply_revocation(signed)
                .map_err(|_| StoreError::Refused),
        }
    }

    fn parse_signed(bytes: &[u8], signature: [u8; 64]) -> StoreResult<RegisteredRequest> {
        if bytes.len() == REVOCATION_BYTES {
            Ok(RegisteredRequest::Revoke(SignedRevocation {
                request: RevocationRequest::parse_canonical(bytes)
                    .map_err(|_| StoreError::Refused)?,
                signature,
            }))
        } else if bytes.len() <= crate::team_authority::signed::v2::MAX_ENVELOPE_BYTES {
            Ok(RegisteredRequest::Policy(SignedRequestV2 {
                request: DeviceRequestV2::parse_canonical(bytes)
                    .map_err(|_| StoreError::Refused)?,
                signature,
            }))
        } else {
            Err(StoreError::Refused)
        }
    }

    fn canonical_bytes(request: &RegisteredRequest) -> Vec<u8> {
        match request {
            RegisteredRequest::Policy(signed) => signed.request.canonical_bytes(),
            RegisteredRequest::Revoke(signed) => signed.request.canonical_bytes(),
        }
    }

    fn signature(request: &RegisteredRequest) -> &[u8; 64] {
        match request {
            RegisteredRequest::Policy(signed) => &signed.signature,
            RegisteredRequest::Revoke(signed) => &signed.signature,
        }
    }

    fn operation(request: &RegisteredRequest) -> &[u8] {
        match request {
            RegisteredRequest::Policy(signed) => signed.request.operation.as_str().as_bytes(),
            RegisteredRequest::Revoke(signed) => signed.request.operation.as_str().as_bytes(),
        }
    }

    fn revision(authority: &RegisteredAuthority) -> u64 {
        authority.revision()
    }

    fn allows(
        authority: &RegisteredAuthority,
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
                Some(RegisteredProfile::ENTRY_SCHEMA.as_bytes()),
            ),
            (
                b"table",
                b"meta",
                b"meta",
                Some(RegisteredProfile::METADATA_SCHEMA.as_bytes()),
            ),
            (b"index", b"sqlite_autoindex_entries_1", b"entries", None),
        ];
        SCHEMA
    }

    fn create_auxiliary(_store: &AuthorityStoreRegistered) -> StoreResult<()> {
        Ok(())
    }

    fn validate_auxiliary(_store: &AuthorityStoreRegistered, _revision: u64) -> StoreResult<()> {
        Ok(())
    }
}

impl AuthorityStoreRegistered {
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
