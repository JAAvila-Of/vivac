use super::*;
use passki::{
    AuthenticationChallenge, AuthenticationCredential, AuthenticationOptions, AuthenticationState,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct ConfirmationContext {
    pub(crate) nonce: [u8; 32],
    pub(crate) issued_at: u64,
    pub(crate) expires_at: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct ConfirmationBinding {
    pub(crate) proposal: [u8; 32],
    pub(crate) registration: [u8; 32],
    pub(crate) context: ConfirmationContext,
}

// Owning either stage is required to advance; public metadata cannot recreate it.
pub(crate) struct ReservedConfirmation {
    registration: UnconfirmedRegistration,
    context: ConfirmationContext,
}

#[derive(serde::Serialize)]
pub(crate) struct FinalAssertionRequest {
    pub(crate) options: AuthenticationChallenge,
    pub(crate) binding: ConfirmationBinding,
    pub(crate) signing_message: String,
}

pub(crate) struct PendingConfirmation {
    registration: UnconfirmedRegistration,
    binding: ConfirmationBinding,
    state: AuthenticationState,
    digest: [u8; 32],
}

// This evidence proves possession only, without admission or authority mutation.
pub(crate) struct PossessionConfirmation {
    registration: UnconfirmedRegistration,
    binding: ConfirmationBinding,
    counter: u32,
    backup_state: bool,
    assertion: Vec<u8>,
    device_signature: Vec<u8>,
}

impl PossessionConfirmation {
    pub(crate) fn registration(&self) -> &UnconfirmedRegistration {
        &self.registration
    }
    pub(crate) fn binding(&self) -> ConfirmationBinding {
        self.binding
    }
    pub(crate) fn counter(&self) -> u32 {
        self.counter
    }
    pub(crate) fn backup_state(&self) -> bool {
        self.backup_state
    }
    pub(crate) fn assertion(&self) -> &[u8] {
        &self.assertion
    }
    pub(crate) fn device_signature(&self) -> &[u8] {
        &self.device_signature
    }
}

type AuthenticationFactory =
    fn(
        &Passki,
        &StoredPasskey,
    ) -> RegistrationResult<(AuthenticationChallenge, AuthenticationState)>;

impl RegistrationVerifier {
    pub(crate) fn reserve_confirmation(
        &self,
        registration: UnconfirmedRegistration,
        proof: &Proof,
        draft: [u8; 32],
    ) -> RegistrationResult<(ConfirmationContext, ReservedConfirmation)> {
        self.reserve_confirmation_using(registration, proof, draft, Runtime::default())
    }

    fn reserve_confirmation_using(
        &self,
        registration: UnconfirmedRegistration,
        proof: &Proof,
        draft: [u8; 32],
        runtime: Runtime,
    ) -> RegistrationResult<(ConfirmationContext, ReservedConfirmation)> {
        self.check_registration(&registration, proof, draft, runtime)?;
        let issued_at = (runtime.clock)()?;
        let mut nonce = [0; 32];
        (runtime.entropy)(&mut nonce)?;
        let context = ConfirmationContext {
            nonce,
            issued_at,
            expires_at: registration.context.expires_at,
        };
        self.check_confirmation_scope(&registration, context, proof, draft, runtime)?;
        Ok((
            context,
            ReservedConfirmation {
                registration,
                context,
            },
        ))
    }

    pub(crate) fn start_confirmation(
        &self,
        reserved: ReservedConfirmation,
        proof: &Proof,
        draft: [u8; 32],
        binding: ConfirmationBinding,
    ) -> RegistrationResult<(FinalAssertionRequest, PendingConfirmation)> {
        self.start_confirmation_using(
            reserved,
            proof,
            draft,
            binding,
            Runtime::default(),
            authentication_factory,
        )
    }

    fn start_confirmation_using(
        &self,
        reserved: ReservedConfirmation,
        proof: &Proof,
        draft: [u8; 32],
        binding: ConfirmationBinding,
        runtime: Runtime,
        factory: AuthenticationFactory,
    ) -> RegistrationResult<(FinalAssertionRequest, PendingConfirmation)> {
        self.check_confirmation_scope(
            &reserved.registration,
            reserved.context,
            proof,
            draft,
            runtime,
        )?;
        if binding.context != reserved.context
            || binding.registration != reserved.registration.commitment
        {
            return Err(RegistrationError::Refused);
        }
        let digest = confirmation_digest(&reserved.registration, proof, binding)?;
        let (mut options, mut state) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                factory(&self.passki, &reserved.registration.stored)
            }))
            .map_err(|_| RegistrationError::Unavailable)??;
        check_authentication_state(&state, &reserved.registration)?;
        if options.rp_id != self.rp
            || options.user_verification != UserVerificationRequirement::Required
            || options.allow_credentials.len() != 1
            || options.allow_credentials[0].type_ != "public-key"
            || options.allow_credentials[0].id
                != encode(&reserved.registration.descriptor.credential)
            || serde_json::to_value(&options.allow_credentials[0].transports)
                .map_err(|_| RegistrationError::Refused)?
                != serde_json::to_value(&reserved.registration.stored.transports)
                    .map_err(|_| RegistrationError::Refused)?
            || options.extensions.is_some()
            || !options.hints.is_empty()
            || state.challenge.len() != 32
            || decode(&options.challenge, 32)? != state.challenge
        {
            return Err(RegistrationError::Refused);
        }
        options.challenge = encode(&digest);
        state.challenge = digest.to_vec();
        let pending = PendingConfirmation {
            registration: reserved.registration,
            binding,
            state,
            digest,
        };
        self.check_pending_confirmation(&pending, proof, draft, binding, runtime)?;
        Ok((
            FinalAssertionRequest {
                options,
                binding,
                signing_message: encode(&signing_message(&digest)),
            },
            pending,
        ))
    }

    pub(crate) fn finish_confirmation(
        &self,
        pending: PendingConfirmation,
        proof: &Proof,
        draft: [u8; 32],
        binding: ConfirmationBinding,
        device_signature: &[u8],
        assertion: &[u8],
    ) -> RegistrationResult<PossessionConfirmation> {
        self.finish_confirmation_using(
            pending,
            proof,
            draft,
            binding,
            FinalEvidence {
                device_signature,
                assertion,
            },
            Runtime::default(),
        )
    }

    fn finish_confirmation_using(
        &self,
        pending: PendingConfirmation,
        proof: &Proof,
        draft: [u8; 32],
        binding: ConfirmationBinding,
        evidence: FinalEvidence<'_>,
        runtime: Runtime,
    ) -> RegistrationResult<PossessionConfirmation> {
        self.check_pending_confirmation(&pending, proof, draft, binding, runtime)?;
        let strict = parse_assertion(evidence.assertion, &pending.digest, &pending.registration)?;
        let key = crate::enrollment_proof::decode_hex::<32>(&proof.challenge.public_key)
            .map_err(|_| RegistrationError::Refused)?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&key)
            .map_err(|_| RegistrationError::Refused)?;
        let signature = ed25519_dalek::Signature::from_slice(evidence.device_signature)
            .map_err(|_| RegistrationError::Refused)?;
        key.verify_strict(&signing_message(&pending.digest), &signature)
            .map_err(|_| RegistrationError::Refused)?;
        let credential: AuthenticationCredential =
            serde_json::from_slice(evidence.assertion).map_err(|_| RegistrationError::Refused)?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.passki.finish_passkey_authentication(
                &credential,
                &pending.state,
                &pending.registration.stored,
            )
        }))
        .map_err(|_| RegistrationError::Unavailable)?
        .map_err(|_| RegistrationError::Refused)?;
        if result.credential_id != pending.registration.descriptor.credential
            || result.counter != strict.counter
            || result.user_handle != strict.user_handle
            || result.prf_first.is_some()
            || result.prf_second.is_some()
            || result.large_blob.is_some()
            || result.large_blob_written.is_some()
        {
            return Err(RegistrationError::Refused);
        }
        self.check_pending_confirmation(&pending, proof, draft, binding, runtime)?;
        Ok(PossessionConfirmation {
            registration: pending.registration,
            binding,
            counter: strict.counter,
            backup_state: strict.backup_state,
            assertion: evidence.assertion.to_vec(),
            device_signature: evidence.device_signature.to_vec(),
        })
    }

    fn check_registration(
        &self,
        registration: &UnconfirmedRegistration,
        proof: &Proof,
        draft: [u8; 32],
        runtime: Runtime,
    ) -> RegistrationResult<()> {
        let now = (runtime.clock)()?;
        proof.verify(now).map_err(|_| RegistrationError::Refused)?;
        if registration.proof != proof_bytes(proof)?
            || registration.draft != draft
            || registration.rp != self.rp
            || registration.origin != self.origin
            || now < registration.context.issued_at
            || now >= registration.context.expires_at
            || registration.context.expires_at != proof.challenge.expires_at
            || (runtime.monotonic)() >= registration.deadline
        {
            return Err(RegistrationError::Refused);
        }
        if !(runtime.cpu)() {
            return Err(RegistrationError::Unavailable);
        }
        let digest = challenge_digest(proof, draft, &self.rp, &self.origin, registration.context)?;
        let mut descriptor =
            parse_response(&registration.response, &digest, &self.rp, &self.origin)?;
        check_stored(&descriptor, &registration.stored)?;
        descriptor.opaque_user_handle = proof.challenge.principal_id.as_bytes().to_vec();
        let mut hash = Sha256::new();
        hash.update(b"vivac.passkey.registration-record.v1\0");
        hash.update(digest);
        hash.update((registration.response.len() as u32).to_be_bytes());
        hash.update(&registration.response);
        if descriptor != registration.descriptor
            || <[u8; 32]>::from(hash.finalize()) != registration.commitment
        {
            return Err(RegistrationError::Refused);
        }
        Ok(())
    }

    fn check_confirmation_scope(
        &self,
        registration: &UnconfirmedRegistration,
        context: ConfirmationContext,
        proof: &Proof,
        draft: [u8; 32],
        runtime: Runtime,
    ) -> RegistrationResult<()> {
        self.check_registration(registration, proof, draft, runtime)?;
        let now = (runtime.clock)()?;
        if context.issued_at < registration.context.issued_at
            || context.expires_at != registration.context.expires_at
            || now < context.issued_at
            || now >= context.expires_at
            || (runtime.monotonic)() >= registration.deadline
        {
            return Err(RegistrationError::Refused);
        }
        Ok(())
    }

    fn check_pending_confirmation(
        &self,
        pending: &PendingConfirmation,
        proof: &Proof,
        draft: [u8; 32],
        binding: ConfirmationBinding,
        runtime: Runtime,
    ) -> RegistrationResult<()> {
        self.check_confirmation_scope(
            &pending.registration,
            pending.binding.context,
            proof,
            draft,
            runtime,
        )?;
        check_authentication_state(&pending.state, &pending.registration)?;
        if binding != pending.binding
            || binding.registration != pending.registration.commitment
            || pending.digest != confirmation_digest(&pending.registration, proof, binding)?
            || pending.state.challenge != pending.digest
        {
            return Err(RegistrationError::Refused);
        }
        Ok(())
    }
}

struct FinalEvidence<'a> {
    device_signature: &'a [u8],
    assertion: &'a [u8],
}

fn check_authentication_state(
    state: &AuthenticationState,
    registration: &UnconfirmedRegistration,
) -> RegistrationResult<()> {
    if state.user_verification != UserVerificationRequirement::Required
        || state.allowed_credentials.as_slice()
            != std::slice::from_ref(&registration.descriptor.credential)
    {
        return Err(RegistrationError::Refused);
    }
    Ok(())
}

fn confirmation_digest(
    registration: &UnconfirmedRegistration,
    proof: &Proof,
    binding: ConfirmationBinding,
) -> RegistrationResult<[u8; 32]> {
    digest_parts(
        proof,
        registration.draft,
        &registration.rp,
        &registration.origin,
        binding,
    )
}

fn digest_parts(
    proof: &Proof,
    draft: [u8; 32],
    rp: &str,
    origin: &str,
    binding: ConfirmationBinding,
) -> RegistrationResult<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(b"vivac.passkey.final-confirmation.v1\0");
    hash.update(binding.registration);
    hash.update(binding.proposal);
    hash.update(proof_bytes(proof)?);
    hash.update(draft);
    for value in [rp, origin] {
        let len = u16::try_from(value.len()).map_err(|_| RegistrationError::Refused)?;
        hash.update(len.to_be_bytes());
        hash.update(value.as_bytes());
    }
    hash.update(binding.context.nonce);
    hash.update(binding.context.issued_at.to_be_bytes());
    hash.update(binding.context.expires_at.to_be_bytes());
    Ok(hash.finalize().into())
}

fn signing_message(digest: &[u8; 32]) -> Vec<u8> {
    let mut message = b"vivac.device.final-confirmation.v1\0".to_vec();
    message.extend_from_slice(digest);
    message
}

fn authentication_factory(
    passki: &Passki,
    stored: &StoredPasskey,
) -> RegistrationResult<(AuthenticationChallenge, AuthenticationState)> {
    let mut options = AuthenticationOptions::default();
    options.user_verification = UserVerificationRequirement::Required;
    passki
        .start_passkey_authentication(std::slice::from_ref(stored), options)
        .map_err(|_| RegistrationError::Refused)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAssertion {
    id: String,
    #[serde(rename = "rawId")]
    raw_id: String,
    #[serde(rename = "type")]
    kind: String,
    response: StrictAssertionResponse,
    #[serde(
        default,
        rename = "clientExtensionResults",
        deserialize_with = "present_empty"
    )]
    _extensions: (),
    #[serde(
        default,
        rename = "authenticatorAttachment",
        deserialize_with = "present_attachment"
    )]
    attachment: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAssertionResponse {
    #[serde(rename = "clientDataJSON")]
    client_data: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    signature: String,
    #[serde(default, rename = "userHandle")]
    user_handle: Option<String>,
}

struct VerifiedAssertion {
    counter: u32,
    backup_state: bool,
    user_handle: Option<Vec<u8>>,
}

fn parse_assertion(
    raw: &[u8],
    digest: &[u8; 32],
    registration: &UnconfirmedRegistration,
) -> RegistrationResult<VerifiedAssertion> {
    if raw.is_empty() || raw.len() > MAX_RESPONSE {
        return Err(RegistrationError::Refused);
    }
    let value: StrictAssertion =
        serde_json::from_slice(raw).map_err(|_| RegistrationError::Refused)?;
    if value.kind != "public-key"
        || value.id != value.raw_id
        || decode(&value.raw_id, MAX_KEY)? != registration.descriptor.credential
        || value
            .attachment
            .as_deref()
            .is_some_and(|hint| !matches!(hint, "platform" | "cross-platform"))
    {
        return Err(RegistrationError::Refused);
    }
    let client: StrictClientData =
        serde_json::from_slice(&decode(&value.response.client_data, MAX_COMPONENT)?)
            .map_err(|_| RegistrationError::Refused)?;
    if client.kind != "webauthn.get"
        || client.cross_origin
        || client.origin != registration.origin
        || decode(&client.challenge, 32)? != digest
    {
        return Err(RegistrationError::Refused);
    }
    let auth = decode(&value.response.authenticator_data, MAX_COMPONENT)?;
    if auth.len() != 37
        || auth[..32] != Sha256::digest(registration.rp.as_bytes())[..]
        || auth[32] & 0xc7 != 0x05
        || auth[32] & 0x22 != 0
        || (auth[32] & 0x08 != 0) != registration.descriptor.backup_eligible
        || (auth[32] & 0x10 != 0 && auth[32] & 0x08 == 0)
        || decode(&value.response.signature, MAX_COMPONENT)?.is_empty()
    {
        return Err(RegistrationError::Refused);
    }
    let user_handle = value
        .response
        .user_handle
        .as_deref()
        .map(|handle| decode(handle, MAX_KEY))
        .transpose()?;
    if user_handle
        .as_ref()
        .is_some_and(|handle| handle != &registration.descriptor.opaque_user_handle)
    {
        return Err(RegistrationError::Refused);
    }
    Ok(VerifiedAssertion {
        counter: u32::from_be_bytes(
            auth[33..37]
                .try_into()
                .map_err(|_| RegistrationError::Refused)?,
        ),
        backup_state: auth[32] & 0x10 != 0,
        user_handle,
    })
}

pub(crate) mod proposal;

#[cfg(test)]
mod tests;
