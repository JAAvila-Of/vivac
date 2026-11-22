use super::*;

const STATE_DOMAIN: &[u8] = b"vivac.passkey.public-state.v1\0";
const CONFIGURATION_DOMAIN: &[u8] = b"vivac.passkey.configuration.v1\0";

// This is public fixture state, never an authenticator secret or admitted trust.
#[derive(Clone, Debug)]
pub(crate) struct PublicPasskeyState {
    organization: OrganizationId,
    principal: PrincipalId,
    passkey: StoredPasskey,
    generation: u64,
}

impl PublicPasskeyState {
    pub(crate) fn new_fixture(
        organization: OrganizationId,
        principal: PrincipalId,
        passkey: StoredPasskey,
    ) -> AssertionResult<Self> {
        if passkey.credential_id.is_empty()
            || passkey.credential_id.len() > MAX_CREDENTIAL_BYTES
            || passkey.public_key.is_empty()
            || passkey.public_key.len() > MAX_PUBLIC_KEY_BYTES
            || !matches!(passkey.algorithm, -8 | -7)
            || (passkey.bs && !passkey.be)
            || passkey.aaguid != [0; 16]
            || !matches!(passkey.attestation_type, passki::AttestationType::None)
            || !passkey.transports.is_empty()
            || passkey.rk.is_some()
            || passkey.large_blob_supported.is_some()
            || passkey.cred_protect.is_some()
            || passkey.min_pin_length.is_some()
        {
            return Err(AssertionError::Refused);
        }
        let mut minimal = StoredPasskey::default();
        minimal.credential_id = passkey.credential_id;
        minimal.public_key = passkey.public_key;
        minimal.algorithm = passkey.algorithm;
        minimal.counter = passkey.counter;
        minimal.be = passkey.be;
        minimal.bs = passkey.bs;
        let binding = TrustedPasskeyBinding::new(organization, principal, minimal)?;
        Ok(Self::from_binding(binding))
    }

    pub(crate) fn organization(&self) -> &OrganizationId {
        &self.organization
    }

    pub(crate) fn principal(&self) -> &PrincipalId {
        &self.principal
    }

    pub(crate) fn credential_id(&self) -> &[u8] {
        &self.passkey.credential_id
    }

    pub(crate) fn counter(&self) -> u32 {
        self.passkey.counter
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn backup_state(&self) -> bool {
        self.passkey.bs
    }

    pub(crate) fn commitment(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(STATE_DOMAIN);
        digest.update(self.organization.as_str().as_bytes());
        digest.update(self.principal.as_str().as_bytes());
        for field in [&self.passkey.credential_id, &self.passkey.public_key] {
            digest.update((field.len() as u16).to_be_bytes());
            digest.update(field);
        }
        digest.update(i64::from(self.passkey.algorithm).to_be_bytes());
        digest.update(self.passkey.counter.to_be_bytes());
        digest.update([u8::from(self.passkey.be), u8::from(self.passkey.bs)]);
        digest.update(self.generation.to_be_bytes());
        digest.finalize().into()
    }

    fn binding(&self) -> AssertionResult<TrustedPasskeyBinding> {
        let mut binding = TrustedPasskeyBinding::new(
            self.organization.clone(),
            self.principal.clone(),
            self.passkey.clone(),
        )?;
        binding.generation = self.generation;
        Ok(binding)
    }

    fn from_binding(binding: TrustedPasskeyBinding) -> Self {
        Self {
            organization: binding.organization,
            principal: binding.principal,
            passkey: binding.passkey,
            generation: binding.generation,
        }
    }
}

// Parsing this public evidence does not establish that verification succeeded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdditionalWitness {
    context: AssertionContext,
    assertion: Vec<u8>,
}

impl AdditionalWitness {
    pub(crate) fn from_untrusted(context: AssertionContext, bytes: &[u8]) -> AssertionResult<Self> {
        if bytes.is_empty()
            || bytes.len() > MAX_ASSERTION_BYTES
            || context
                .expires_at
                .checked_sub(context.issued_at)
                .is_none_or(|window| window == 0 || window > 300)
        {
            return Err(AssertionError::Refused);
        }
        Ok(Self {
            context,
            assertion: bytes.to_vec(),
        })
    }

    pub(crate) fn context(&self) -> AssertionContext {
        self.context
    }

    pub(crate) fn assertion(&self) -> &[u8] {
        &self.assertion
    }
}

// The original live ceremony and its binding stay owned, private and ephemeral.
pub(crate) struct OwnedAdditionalChallenge {
    binding: TrustedPasskeyBinding,
    pending: PendingAssertion,
    state: [u8; 32],
}

impl EnrollmentAssertionVerifier {
    pub(crate) fn configuration_commitment(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(CONFIGURATION_DOMAIN);
        for field in [self.rp_id.as_bytes(), self.origin.as_bytes()] {
            digest.update((field.len() as u16).to_be_bytes());
            digest.update(field);
        }
        digest.finalize().into()
    }

    pub(crate) fn start_recorded_additional(
        &self,
        proof: &Proof,
        state: &PublicPasskeyState,
        context_digest: [u8; 32],
    ) -> AssertionResult<(AssertionRequest, OwnedAdditionalChallenge)> {
        check_owner(proof, state)?;
        let binding = state.binding()?;
        let (request, pending) = self.start_additional(proof, &binding, context_digest)?;
        Ok((
            request,
            OwnedAdditionalChallenge {
                binding,
                pending,
                state: state.commitment(),
            },
        ))
    }

    pub(crate) fn finish_recorded_additional(
        &self,
        pending: OwnedAdditionalChallenge,
        proof: &Proof,
        current: &PublicPasskeyState,
        assertion: &[u8],
        context_digest: [u8; 32],
        now: u64,
    ) -> AssertionResult<(AdditionalWitness, PublicPasskeyState)> {
        if !supported_cpu() {
            return Err(AssertionError::Unavailable);
        }
        check_owner(proof, current)?;
        if pending.state != current.commitment() {
            return Err(AssertionError::Refused);
        }
        let OwnedAdditionalChallenge {
            mut binding,
            pending,
            ..
        } = pending;
        let context = pending.context;
        let witness = AdditionalWitness::from_untrusted(context, assertion)?;
        self.prepare_scoped_at(
            pending,
            proof,
            &mut binding,
            assertion,
            AssertionScope::AdditionalDevice(context_digest),
            now,
        )?
        .publish();
        Ok((witness, PublicPasskeyState::from_binding(binding)))
    }

    pub(crate) fn replay_recorded_additional(
        &self,
        proof: &Proof,
        current: &PublicPasskeyState,
        witness: &AdditionalWitness,
        context_digest: [u8; 32],
        accepted_at: u64,
    ) -> AssertionResult<PublicPasskeyState> {
        if !supported_cpu() {
            return Err(AssertionError::Unavailable);
        }
        check_owner(proof, current)?;
        proof
            .verify(accepted_at)
            .map_err(|_| AssertionError::Refused)?;
        if witness.context.issued_at < proof.challenge.issued_at
            || witness.context.expires_at != proof.challenge.expires_at
        {
            return Err(AssertionError::Refused);
        }
        let mut binding = current.binding()?;
        let (challenge, mut authentication) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                system_ceremony(&self.passki, &binding.passkey)
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
        authentication.challenge = additional_commitment(
            &proof.challenge,
            &binding.organization,
            &binding.principal,
            &binding.passkey.credential_id,
            &self.rp_id,
            &self.origin,
            &witness.context,
            context_digest,
        )?
        .to_vec();
        let pending = PendingAssertion {
            scope: AssertionScope::AdditionalDevice(context_digest),
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
            context: witness.context,
        };
        self.prepare_scoped_at(
            pending,
            proof,
            &mut binding,
            witness.assertion(),
            AssertionScope::AdditionalDevice(context_digest),
            accepted_at,
        )?
        .publish();
        Ok(PublicPasskeyState::from_binding(binding))
    }
}

fn check_owner(proof: &Proof, state: &PublicPasskeyState) -> AssertionResult<()> {
    if proof.challenge.organization_id != state.organization.as_str()
        || proof.challenge.principal_id != state.principal.as_str()
    {
        return Err(AssertionError::Refused);
    }
    Ok(())
}

#[cfg(test)]
#[path = "durable/tests.rs"]
mod tests;
