use crate::enrollment_proof::{Challenge, Proof};
use crate::team_authority::{OrganizationId, PrincipalId};
use base64ct::{Base64UrlUnpadded, Encoding};
use passki::{
    AuthenticationChallenge, AuthenticationCredential, AuthenticationOptions, AuthenticationState,
    Passki, StoredPasskey, UserVerificationRequirement,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

const MAX_ASSERTION_BYTES: usize = 16 * 1024;
const MAX_CLIENT_DATA_BYTES: usize = 8 * 1024;
const MAX_CREDENTIAL_BYTES: usize = 1024;
const MAX_PUBLIC_KEY_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AssertionError {
    Refused,
    Unavailable,
}

type AssertionResult<T> = Result<T, AssertionError>;

pub(crate) struct EnrollmentAssertionVerifier {
    passki: Passki,
    rp_id: String,
    origin: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct AssertionContext {
    pub nonce: [u8; 32],
    pub issued_at: u64,
    pub expires_at: u64,
}

#[derive(serde::Serialize)]
pub(crate) struct AssertionRequest {
    pub options: AuthenticationChallenge,
    pub context: AssertionContext,
}

// These public credentials are externally trusted fixtures, not an enrollment API.
pub(crate) struct TrustedPasskeyBinding {
    organization: OrganizationId,
    principal: PrincipalId,
    passkey: StoredPasskey,
    expected_user_handle: Vec<u8>,
    instance: [u8; 32],
    generation: u64,
}

impl TrustedPasskeyBinding {
    pub(crate) fn organization(&self) -> &OrganizationId {
        &self.organization
    }

    pub(crate) fn principal(&self) -> &PrincipalId {
        &self.principal
    }

    pub(crate) fn new(
        organization: OrganizationId,
        principal: PrincipalId,
        passkey: StoredPasskey,
    ) -> AssertionResult<Self> {
        Self::new_using(organization, principal, passkey, system_entropy)
    }

    fn new_using(
        organization: OrganizationId,
        principal: PrincipalId,
        passkey: StoredPasskey,
        entropy: fn(&mut [u8]) -> AssertionResult<()>,
    ) -> AssertionResult<Self> {
        if passkey.credential_id.is_empty()
            || passkey.credential_id.len() > MAX_CREDENTIAL_BYTES
            || passkey.public_key.is_empty()
            || passkey.public_key.len() > MAX_PUBLIC_KEY_BYTES
            || !matches!(passkey.algorithm, -8 | -7)
            || (passkey.bs && !passkey.be)
        {
            return Err(AssertionError::Refused);
        }
        let mut instance = [0; 32];
        entropy(&mut instance)?;
        Ok(Self {
            expected_user_handle: principal.as_str().as_bytes().to_vec(),
            organization,
            principal,
            passkey,
            instance,
            generation: 0,
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum AssertionScope {
    Enrollment,
    AdditionalDevice([u8; 32]),
}

// Server-owned ceremony state is deliberately neither cloneable nor serializable.
pub(crate) struct PendingAssertion {
    scope: AssertionScope,
    authentication: AuthenticationState,
    enrollment: Vec<u8>,
    organization: OrganizationId,
    principal: PrincipalId,
    credential: Vec<u8>,
    instance: [u8; 32],
    generation: u64,
    rp_id: String,
    origin: String,
    context: AssertionContext,
}

// A scoped assertion is not human approval, membership or admission evidence.
pub(crate) struct AssertionEvidence {
    enrollment: Vec<u8>,
    organization: OrganizationId,
    principal: PrincipalId,
    credential: Vec<u8>,
    counter: u32,
}

// Verification retains exclusive access until the caller publishes or discards it.
pub(crate) struct PreparedAssertion<'binding> {
    binding: &'binding mut TrustedPasskeyBinding,
    counter: u32,
    backup_state: bool,
    generation: u64,
    evidence: AssertionEvidence,
}

impl PreparedAssertion<'_> {
    pub(crate) fn publish(self) -> AssertionEvidence {
        self.binding.passkey.counter = self.counter;
        self.binding.passkey.bs = self.backup_state;
        self.binding.generation = self.generation;
        self.evidence
    }
}

impl EnrollmentAssertionVerifier {
    pub(crate) fn new(rp_id: &str, origin: &str) -> AssertionResult<Self> {
        if rp_id.is_empty() || rp_id.len() > 253 || origin.len() > 2048 || origin.contains('@') {
            return Err(AssertionError::Refused);
        }
        let origin = Url::parse(origin).map_err(|_| AssertionError::Refused)?;
        if origin.scheme() != "https"
            || origin.host_str() != Some(rp_id)
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(AssertionError::Refused);
        }
        let mut canonical_origin = origin.origin().ascii_serialization();
        if canonical_origin.ends_with('/') {
            canonical_origin.pop();
        }
        Ok(Self {
            passki: Passki::new(rp_id, &[&canonical_origin], "Vivac assertion probe"),
            rp_id: rp_id.into(),
            origin: canonical_origin,
        })
    }

    pub(crate) fn start(
        &self,
        proof: &Proof,
        binding: &TrustedPasskeyBinding,
    ) -> AssertionResult<(AssertionRequest, PendingAssertion)> {
        self.start_using(proof, binding, system_time, supported_cpu, system_ceremony)
    }

    pub(crate) fn finish(
        &self,
        pending: PendingAssertion,
        proof: &Proof,
        binding: &mut TrustedPasskeyBinding,
        assertion: &[u8],
    ) -> AssertionResult<AssertionEvidence> {
        self.prepare(pending, proof, binding, assertion)
            .map(PreparedAssertion::publish)
    }

    pub(crate) fn start_additional(
        &self,
        proof: &Proof,
        binding: &TrustedPasskeyBinding,
        context_digest: [u8; 32],
    ) -> AssertionResult<(AssertionRequest, PendingAssertion)> {
        self.start_scoped_using(
            proof,
            binding,
            system_time,
            supported_cpu,
            system_ceremony,
            AssertionScope::AdditionalDevice(context_digest),
        )
    }

    pub(crate) fn prepare_additional<'binding>(
        &self,
        pending: PendingAssertion,
        proof: &Proof,
        binding: &'binding mut TrustedPasskeyBinding,
        assertion: &[u8],
        context_digest: [u8; 32],
    ) -> AssertionResult<PreparedAssertion<'binding>> {
        self.prepare_scoped_using(
            pending,
            proof,
            binding,
            assertion,
            AssertionScope::AdditionalDevice(context_digest),
            (system_time, supported_cpu),
        )
    }

    pub(crate) fn prepare<'binding>(
        &self,
        pending: PendingAssertion,
        proof: &Proof,
        binding: &'binding mut TrustedPasskeyBinding,
        assertion: &[u8],
    ) -> AssertionResult<PreparedAssertion<'binding>> {
        self.prepare_using(
            pending,
            proof,
            binding,
            assertion,
            system_time,
            supported_cpu,
        )
    }

    fn start_using(
        &self,
        proof: &Proof,
        binding: &TrustedPasskeyBinding,
        clock: fn() -> AssertionResult<u64>,
        cpu: fn() -> bool,
        ceremony: CeremonyGenerator,
    ) -> AssertionResult<(AssertionRequest, PendingAssertion)> {
        self.start_scoped_using(
            proof,
            binding,
            clock,
            cpu,
            ceremony,
            AssertionScope::Enrollment,
        )
    }

    fn start_scoped_using(
        &self,
        proof: &Proof,
        binding: &TrustedPasskeyBinding,
        clock: fn() -> AssertionResult<u64>,
        cpu: fn() -> bool,
        ceremony: CeremonyGenerator,
        scope: AssertionScope,
    ) -> AssertionResult<(AssertionRequest, PendingAssertion)> {
        if !cpu() {
            return Err(AssertionError::Unavailable);
        }
        let now = clock()?;
        proof.verify(now).map_err(|_| AssertionError::Refused)?;
        if proof.challenge.organization_id != binding.organization.as_str() {
            return Err(AssertionError::Refused);
        }
        let (mut challenge, mut authentication) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ceremony(&self.passki, &binding.passkey)
            }))
            .map_err(|_| AssertionError::Unavailable)??;
        if authentication.challenge.len() != 32
            || authentication.allowed_credentials != [binding.passkey.credential_id.clone()]
            || authentication.user_verification != UserVerificationRequirement::Required
            || challenge.user_verification != UserVerificationRequirement::Required
            || challenge.challenge != encode(&authentication.challenge)
            || challenge.rp_id != self.rp_id
            || challenge.extensions.is_some()
        {
            return Err(AssertionError::Refused);
        }
        let context = AssertionContext {
            nonce: authentication
                .challenge
                .as_slice()
                .try_into()
                .map_err(|_| AssertionError::Refused)?,
            issued_at: now,
            expires_at: proof.challenge.expires_at,
        };
        let digest = scoped_commitment(
            scope,
            commitment(
                &proof.challenge,
                &binding.organization,
                &binding.principal,
                &binding.passkey.credential_id,
                &self.rp_id,
                &self.origin,
                &context,
            )?,
        );
        authentication.challenge = digest.to_vec();
        challenge.challenge = encode(&digest);
        let pending = PendingAssertion {
            scope,
            authentication,
            enrollment: proof
                .challenge
                .canonical_bytes()
                .map_err(|_| AssertionError::Refused)?,
            organization: binding.organization.clone(),
            principal: binding.principal.clone(),
            credential: binding.passkey.credential_id.clone(),
            instance: binding.instance,
            generation: binding.generation,
            rp_id: self.rp_id.clone(),
            origin: self.origin.clone(),
            context,
        };
        Ok((
            AssertionRequest {
                options: challenge,
                context,
            },
            pending,
        ))
    }

    fn finish_using(
        &self,
        pending: PendingAssertion,
        proof: &Proof,
        binding: &mut TrustedPasskeyBinding,
        assertion: &[u8],
        clock: fn() -> AssertionResult<u64>,
        cpu: fn() -> bool,
    ) -> AssertionResult<AssertionEvidence> {
        self.prepare_using(pending, proof, binding, assertion, clock, cpu)
            .map(PreparedAssertion::publish)
    }

    fn prepare_using<'binding>(
        &self,
        pending: PendingAssertion,
        proof: &Proof,
        binding: &'binding mut TrustedPasskeyBinding,
        assertion: &[u8],
        clock: fn() -> AssertionResult<u64>,
        cpu: fn() -> bool,
    ) -> AssertionResult<PreparedAssertion<'binding>> {
        self.prepare_scoped_using(
            pending,
            proof,
            binding,
            assertion,
            AssertionScope::Enrollment,
            (clock, cpu),
        )
    }

    fn prepare_scoped_using<'binding>(
        &self,
        pending: PendingAssertion,
        proof: &Proof,
        binding: &'binding mut TrustedPasskeyBinding,
        assertion: &[u8],
        scope: AssertionScope,
        runtime: AssertionRuntime,
    ) -> AssertionResult<PreparedAssertion<'binding>> {
        let (clock, cpu) = runtime;
        if !cpu() {
            return Err(AssertionError::Unavailable);
        }
        let now = clock()?;
        self.prepare_scoped_at(pending, proof, binding, assertion, scope, now)
    }

    fn prepare_scoped_at<'binding>(
        &self,
        pending: PendingAssertion,
        proof: &Proof,
        binding: &'binding mut TrustedPasskeyBinding,
        assertion: &[u8],
        scope: AssertionScope,
        now: u64,
    ) -> AssertionResult<PreparedAssertion<'binding>> {
        proof.verify(now).map_err(|_| AssertionError::Refused)?;
        if pending.scope != scope
            || now < pending.context.issued_at
            || now >= pending.context.expires_at
            || proof
                .challenge
                .canonical_bytes()
                .map_err(|_| AssertionError::Refused)?
                != pending.enrollment
            || pending.organization != binding.organization
            || pending.principal != binding.principal
            || pending.credential != binding.passkey.credential_id
            || pending.instance != binding.instance
            || pending.generation != binding.generation
            || pending.rp_id != self.rp_id
            || pending.origin != self.origin
            || pending.authentication.user_verification != UserVerificationRequirement::Required
            || pending.authentication.allowed_credentials != [binding.passkey.credential_id.clone()]
        {
            return Err(AssertionError::Refused);
        }
        if pending.authentication.challenge
            != scoped_commitment(
                scope,
                commitment(
                    &proof.challenge,
                    &binding.organization,
                    &binding.principal,
                    &binding.passkey.credential_id,
                    &self.rp_id,
                    &self.origin,
                    &pending.context,
                )?,
            )
        {
            return Err(AssertionError::Refused);
        }
        let next_generation = binding
            .generation
            .checked_add(1)
            .ok_or(AssertionError::Refused)?;
        let backup_state = self.validate_assertion(assertion, &pending, binding)?;
        let credential: AuthenticationCredential =
            serde_json::from_slice(assertion).map_err(|_| AssertionError::Refused)?;
        let result = self
            .passki
            .finish_passkey_authentication(&credential, &pending.authentication, &binding.passkey)
            .map_err(|_| AssertionError::Refused)?;
        if result.credential_id != binding.passkey.credential_id
            || result
                .user_handle
                .as_ref()
                .is_some_and(|handle| handle != &binding.expected_user_handle)
        {
            return Err(AssertionError::Refused);
        }
        let evidence = AssertionEvidence {
            enrollment: pending.enrollment,
            organization: binding.organization.clone(),
            principal: binding.principal.clone(),
            credential: binding.passkey.credential_id.clone(),
            counter: result.counter,
        };
        Ok(PreparedAssertion {
            binding,
            counter: result.counter,
            backup_state,
            generation: next_generation,
            evidence,
        })
    }

    fn validate_assertion(
        &self,
        bytes: &[u8],
        pending: &PendingAssertion,
        binding: &TrustedPasskeyBinding,
    ) -> AssertionResult<bool> {
        if bytes.is_empty() || bytes.len() > MAX_ASSERTION_BYTES {
            return Err(AssertionError::Refused);
        }
        let input: StrictAssertion =
            serde_json::from_slice(bytes).map_err(|_| AssertionError::Refused)?;
        if input.kind != "public-key"
            || input.id != input.raw_id
            || decode(&input.raw_id, MAX_CREDENTIAL_BYTES)? != binding.passkey.credential_id
            || input
                .authenticator_attachment
                .as_deref()
                .is_some_and(|value| !matches!(value, "platform" | "cross-platform"))
        {
            return Err(AssertionError::Refused);
        }
        if let Some(handle) = &input.response.user_handle {
            if decode(handle, 64)? != binding.expected_user_handle {
                return Err(AssertionError::Refused);
            }
        }
        let client_bytes = decode(&input.response.client_data_json, MAX_CLIENT_DATA_BYTES)?;
        let client: StrictClientData =
            serde_json::from_slice(&client_bytes).map_err(|_| AssertionError::Refused)?;
        if client.kind != "webauthn.get"
            || client.origin != self.origin
            || client.cross_origin
            || decode(&client.challenge, 32)? != pending.authentication.challenge
        {
            return Err(AssertionError::Refused);
        }
        let data = decode(&input.response.authenticator_data, 37)?;
        if data.len() != 37 {
            return Err(AssertionError::Refused);
        }
        let flags = data[32];
        let backup_eligible = flags & 0x08 != 0;
        let backup_state = flags & 0x10 != 0;
        if flags & !0x1d != 0
            || flags & 0x05 != 0x05
            || backup_eligible != binding.passkey.be
            || (backup_state && !backup_eligible)
        {
            return Err(AssertionError::Refused);
        }
        let signature = decode(&input.response.signature, 128)?;
        if (binding.passkey.algorithm == -8 && signature.len() != 64)
            || (binding.passkey.algorithm == -7 && !(8..=80).contains(&signature.len()))
        {
            return Err(AssertionError::Refused);
        }
        Ok(backup_state)
    }
}

fn scoped_commitment(scope: AssertionScope, commitment: [u8; 32]) -> [u8; 32] {
    match scope {
        AssertionScope::Enrollment => commitment,
        AssertionScope::AdditionalDevice(context) => {
            let mut digest = Sha256::new();
            digest.update(b"vivac.device.additional-assertion.v1\0");
            digest.update(commitment);
            digest.update(context);
            digest.finalize().into()
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn additional_commitment(
    candidate: &Challenge,
    organization: &OrganizationId,
    principal: &PrincipalId,
    credential: &[u8],
    rp_id: &str,
    origin: &str,
    context: &AssertionContext,
    context_digest: [u8; 32],
) -> AssertionResult<[u8; 32]> {
    Ok(scoped_commitment(
        AssertionScope::AdditionalDevice(context_digest),
        commitment(
            candidate,
            organization,
            principal,
            credential,
            rp_id,
            origin,
            context,
        )?,
    ))
}

pub(crate) fn commitment(
    candidate: &Challenge,
    organization: &OrganizationId,
    principal: &PrincipalId,
    credential: &[u8],
    rp_id: &str,
    origin: &str,
    context: &AssertionContext,
) -> AssertionResult<[u8; 32]> {
    candidate
        .validate(context.issued_at)
        .map_err(|_| AssertionError::Refused)?;
    if credential.is_empty()
        || credential.len() > MAX_CREDENTIAL_BYTES
        || rp_id.is_empty()
        || rp_id.len() > 253
        || origin.is_empty()
        || origin.len() > 2048
        || context.expires_at != candidate.expires_at
        || context.issued_at >= context.expires_at
    {
        return Err(AssertionError::Refused);
    }
    let mut digest = Sha256::new();
    digest.update(b"vivac.device.enrollment-assertion.v1\0");
    digest.update(
        candidate
            .canonical_bytes()
            .map_err(|_| AssertionError::Refused)?,
    );
    digest.update(organization.as_str().as_bytes());
    digest.update(principal.as_str().as_bytes());
    for field in [credential, rp_id.as_bytes(), origin.as_bytes()] {
        digest.update(
            u16::try_from(field.len())
                .map_err(|_| AssertionError::Refused)?
                .to_be_bytes(),
        );
        digest.update(field);
    }
    digest.update(context.nonce);
    digest.update(context.issued_at.to_be_bytes());
    digest.update(context.expires_at.to_be_bytes());
    Ok(digest.finalize().into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAssertion {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "rawId")]
    raw_id: String,
    response: StrictResponse,
    #[serde(default, rename = "clientExtensionResults")]
    _extensions: EmptyExtensions,
    #[serde(default, rename = "authenticatorAttachment")]
    authenticator_attachment: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyExtensions {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictResponse {
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    signature: String,
    #[serde(default, rename = "userHandle")]
    user_handle: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(default, rename = "crossOrigin")]
    cross_origin: bool,
}

type CeremonyGenerator =
    fn(&Passki, &StoredPasskey) -> AssertionResult<(AuthenticationChallenge, AuthenticationState)>;

type AssertionRuntime = (fn() -> AssertionResult<u64>, fn() -> bool);

fn system_ceremony(
    passki: &Passki,
    passkey: &StoredPasskey,
) -> AssertionResult<(AuthenticationChallenge, AuthenticationState)> {
    let mut options = AuthenticationOptions::default();
    options.user_verification = UserVerificationRequirement::Required;
    passki
        .start_passkey_authentication(std::slice::from_ref(passkey), options)
        .map_err(|_| AssertionError::Refused)
}

fn system_time() -> AssertionResult<u64> {
    crate::enrollment_proof::current_time().map_err(|_| AssertionError::Unavailable)
}

fn system_entropy(bytes: &mut [u8]) -> AssertionResult<()> {
    getrandom::getrandom(bytes).map_err(|_| AssertionError::Unavailable)
}

fn encode(bytes: &[u8]) -> String {
    Base64UrlUnpadded::encode_string(bytes)
}

fn decode(value: &str, limit: usize) -> AssertionResult<Vec<u8>> {
    let bytes = Base64UrlUnpadded::decode_vec(value).map_err(|_| AssertionError::Refused)?;
    if bytes.len() > limit || encode(&bytes) != value {
        return Err(AssertionError::Refused);
    }
    Ok(bytes)
}

fn supported_cpu() -> bool {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        std::is_x86_feature_detected!("aes")
            && std::is_x86_feature_detected!("ssse3")
            && std::is_x86_feature_detected!("avx")
            && std::is_x86_feature_detected!("avx2")
            && std::is_x86_feature_detected!("adx")
            && std::is_x86_feature_detected!("bmi2")
            && std::is_x86_feature_detected!("pclmulqdq")
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        false
    }
}

#[cfg(test)]
#[path = "approval/tests.rs"]
pub(crate) mod tests;

#[path = "approval/durable.rs"]
pub(crate) mod durable;
