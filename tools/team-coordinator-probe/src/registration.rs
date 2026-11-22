use crate::enrollment_proof::Proof;
use base64ct::{Base64UrlUnpadded, Encoding};
use ciborium::Value;
use passki::{
    AttestationType, Passki, RegistrationChallenge, RegistrationCredential, RegistrationOptions,
    RegistrationState, ResidentKeyRequirement, StoredPasskey, UserVerificationRequirement,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::time::{Duration, Instant};

const MAX_RESPONSE: usize = 16 * 1024;
const MAX_COMPONENT: usize = 8 * 1024;
const MAX_KEY: usize = 1024;
const CHROMIUM_CLIENT_DATA_MARKER: &str =
    "do not compare clientDataJSON against a template. See https://goo.gl/yabPex";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegistrationError {
    Refused,
    Unavailable,
}
type RegistrationResult<T> = Result<T, RegistrationError>;

pub(crate) struct RegistrationVerifier {
    passki: Passki,
    rp: String,
    origin: String,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(crate) struct RegistrationContext {
    nonce: [u8; 32],
    issued_at: u64,
    expires_at: u64,
}

#[derive(serde::Serialize)]
pub(crate) struct RegistrationRequest {
    pub(crate) options: RegistrationChallenge,
    pub(crate) context: RegistrationContext,
}

// The server retains this single-use state; no archive can recreate it.
pub(crate) struct PendingRegistration {
    state: RegistrationState,
    proof: Vec<u8>,
    review: [u8; 32],
    rp: String,
    origin: String,
    context: RegistrationContext,
    digest: [u8; 32],
    deadline: Instant,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct RegistrationDescriptor {
    aaguid: [u8; 16],
    counter: u32,
    backup_eligible: bool,
    backup_state: bool,
    algorithm: i32,
    public_key: Vec<u8>,
    credential: Vec<u8>,
    transports: Vec<String>,
    attachment: Option<String>,
    opaque_user_handle: Vec<u8>,
}

// This is an unconfirmed public descriptor, never authority or an enrolled binding.
pub(crate) struct UnconfirmedRegistration {
    descriptor: RegistrationDescriptor,
    response: Vec<u8>,
    context: RegistrationContext,
    commitment: [u8; 32],
    stored: StoredPasskey,
    proof: Vec<u8>,
    draft: [u8; 32],
    rp: String,
    origin: String,
    deadline: Instant,
}

impl UnconfirmedRegistration {
    pub(crate) fn commitment(&self) -> [u8; 32] {
        self.commitment
    }
    pub(crate) fn descriptor(&self) -> &RegistrationDescriptor {
        &self.descriptor
    }
}

type Factory = fn(&Passki, &[u8]) -> RegistrationResult<(RegistrationChallenge, RegistrationState)>;
#[derive(Clone, Copy)]
struct Runtime {
    clock: fn() -> RegistrationResult<u64>,
    monotonic: fn() -> Instant,
    entropy: fn(&mut [u8]) -> RegistrationResult<()>,
    cpu: fn() -> bool,
    factory: Factory,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            clock: system_time,
            monotonic: Instant::now,
            entropy: system_entropy,
            cpu: supported_cpu,
            factory: system_factory,
        }
    }
}

impl RegistrationVerifier {
    pub(crate) fn new(rp: &str, origin: &str) -> RegistrationResult<Self> {
        if rp.is_empty() || rp.len() > 253 || origin.len() > 2048 || origin.contains('@') {
            return Err(RegistrationError::Refused);
        }
        let url = url::Url::parse(origin).map_err(|_| RegistrationError::Refused)?;
        if url.scheme() != "https"
            || url.host_str() != Some(rp)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(RegistrationError::Refused);
        }
        let origin = url.origin().ascii_serialization();
        Ok(Self {
            passki: Passki::new(rp, &[&origin], "Vivac registration probe"),
            rp: rp.to_owned(),
            origin,
        })
    }

    pub(crate) fn start(
        &self,
        proof: &Proof,
        review: [u8; 32],
    ) -> RegistrationResult<(RegistrationRequest, PendingRegistration)> {
        self.start_using(proof, review, Runtime::default())
    }

    fn start_using(
        &self,
        proof: &Proof,
        review: [u8; 32],
        runtime: Runtime,
    ) -> RegistrationResult<(RegistrationRequest, PendingRegistration)> {
        let now = (runtime.clock)()?;
        proof.verify(now).map_err(|_| RegistrationError::Refused)?;
        if !(runtime.cpu)() {
            return Err(RegistrationError::Unavailable);
        }
        let deadline = (runtime.monotonic)()
            .checked_add(Duration::from_secs(
                proof
                    .challenge
                    .expires_at
                    .checked_sub(now)
                    .ok_or(RegistrationError::Refused)?,
            ))
            .ok_or(RegistrationError::Refused)?;
        let mut nonce = [0; 32];
        (runtime.entropy)(&mut nonce)?;
        let context = RegistrationContext {
            nonce,
            issued_at: now,
            expires_at: proof.challenge.expires_at,
        };
        let digest = challenge_digest(proof, review, &self.rp, &self.origin, context)?;
        let (mut options, mut state) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (runtime.factory)(&self.passki, proof.challenge.principal_id.as_bytes())
            }))
            .map_err(|_| RegistrationError::Unavailable)??;
        options
            .pub_key_cred_params
            .retain(|param| matches!(param.alg, -8 | -7));
        if options.rp.id != self.rp
            || options.pub_key_cred_params.len() != 2
            || options.pub_key_cred_params[0].alg != -8
            || options.pub_key_cred_params[1].alg != -7
            || options
                .pub_key_cred_params
                .iter()
                .any(|param| param.type_ != "public-key")
            || state.user_verification != UserVerificationRequirement::Required
            || state.required_cred_protect.is_some()
            || state.user.id != encode(proof.challenge.principal_id.as_bytes())
            || options.user.id != state.user.id
            || options.user.name != state.user.name
            || options.user.display_name != state.user.display_name
            || options.authenticator_selection.user_verification
                != UserVerificationRequirement::Required
            || !matches!(
                &options.authenticator_selection.resident_key,
                ResidentKeyRequirement::Discouraged
            )
            || !matches!(
                &options.attestation,
                passki::AttestationConveyancePreference::None
            )
            || options.extensions.is_some()
            || !options.hints.is_empty()
            || !options.exclude_credentials.is_empty()
            || options
                .authenticator_selection
                .authenticator_attachment
                .is_some()
        {
            return Err(RegistrationError::Refused);
        }
        options.challenge = encode(&digest);
        state.challenge = digest.to_vec();
        let pending = PendingRegistration {
            state,
            proof: proof_bytes(proof)?,
            review,
            rp: self.rp.clone(),
            origin: self.origin.clone(),
            context,
            digest,
            deadline,
        };
        self.check(&pending, proof, review, runtime)?;
        Ok((RegistrationRequest { options, context }, pending))
    }

    pub(crate) fn finish(
        &self,
        pending: PendingRegistration,
        proof: &Proof,
        review: [u8; 32],
        response: &[u8],
    ) -> RegistrationResult<UnconfirmedRegistration> {
        self.finish_using(pending, proof, review, response, Runtime::default())
    }

    fn check(
        &self,
        pending: &PendingRegistration,
        proof: &Proof,
        review: [u8; 32],
        runtime: Runtime,
    ) -> RegistrationResult<()> {
        let now = (runtime.clock)()?;
        proof.verify(now).map_err(|_| RegistrationError::Refused)?;
        if now < pending.context.issued_at
            || now >= pending.context.expires_at
            || (runtime.monotonic)() >= pending.deadline
            || pending.rp != self.rp
            || pending.origin != self.origin
            || pending.review != review
            || pending.state.user.id != encode(proof.challenge.principal_id.as_bytes())
            || pending.state.user_verification != UserVerificationRequirement::Required
            || pending.state.required_cred_protect.is_some()
            || pending.proof != proof_bytes(proof)?
            || pending.digest
                != challenge_digest(proof, review, &self.rp, &self.origin, pending.context)?
            || pending.state.challenge != pending.digest
        {
            return Err(RegistrationError::Refused);
        }
        if !(runtime.cpu)() {
            return Err(RegistrationError::Unavailable);
        }
        Ok(())
    }

    fn finish_using(
        &self,
        pending: PendingRegistration,
        proof: &Proof,
        review: [u8; 32],
        response: &[u8],
        runtime: Runtime,
    ) -> RegistrationResult<UnconfirmedRegistration> {
        self.check(&pending, proof, review, runtime)?;
        let mut descriptor = parse_response(response, &pending.digest, &self.rp, &self.origin)?;
        let credential: RegistrationCredential =
            serde_json::from_slice(response).map_err(|_| RegistrationError::Refused)?;
        let stored = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.passki
                .finish_passkey_registration(&credential, &pending.state)
        }))
        .map_err(|_| RegistrationError::Unavailable)?
        .map_err(|_| RegistrationError::Refused)?;
        check_stored(&descriptor, &stored)?;
        self.check(&pending, proof, review, runtime)?;
        descriptor.opaque_user_handle = decode(&pending.state.user.id, 1024)?;
        let mut hash = Sha256::new();
        hash.update(b"vivac.passkey.registration-record.v1\0");
        hash.update(pending.digest);
        hash.update((response.len() as u32).to_be_bytes());
        hash.update(response);
        Ok(UnconfirmedRegistration {
            descriptor,
            response: response.to_vec(),
            context: pending.context,
            commitment: hash.finalize().into(),
            stored,
            proof: pending.proof,
            draft: pending.review,
            rp: pending.rp,
            origin: pending.origin,
            deadline: pending.deadline,
        })
    }
}

fn proof_bytes(proof: &Proof) -> RegistrationResult<Vec<u8>> {
    let mut bytes = proof
        .challenge
        .canonical_bytes()
        .map_err(|_| RegistrationError::Refused)?;
    bytes.extend_from_slice(
        &crate::enrollment_proof::decode_hex::<64>(&proof.signature)
            .map_err(|_| RegistrationError::Refused)?,
    );
    Ok(bytes)
}
fn challenge_digest(
    proof: &Proof,
    review: [u8; 32],
    rp: &str,
    origin: &str,
    context: RegistrationContext,
) -> RegistrationResult<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(b"vivac.passkey.registration-challenge.v1\0");
    hash.update(proof_bytes(proof)?);
    hash.update(review);
    for value in [rp, origin] {
        hash.update((value.len() as u16).to_be_bytes());
        hash.update(value);
    }
    hash.update(context.nonce);
    hash.update(context.issued_at.to_be_bytes());
    hash.update(context.expires_at.to_be_bytes());
    Ok(hash.finalize().into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCredential {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "rawId")]
    raw_id: String,
    response: StrictResponse,
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
struct StrictResponse {
    #[serde(rename = "clientDataJSON")]
    client_data: String,
    #[serde(rename = "attestationObject")]
    attestation: String,
    #[serde(default)]
    transports: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyExtensions {}
fn present_empty<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    EmptyExtensions::deserialize(deserializer).map(|_| ())
}
fn present_attachment<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
fn present_chromium_marker<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<(), D::Error> {
    let marker = String::deserialize(deserializer)?;
    if marker != CHROMIUM_CLIENT_DATA_MARKER {
        return Err(serde::de::Error::custom(
            "invalid Chromium client data marker",
        ));
    }
    Ok(())
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
    #[serde(
        default,
        rename = "other_keys_can_be_added_here",
        deserialize_with = "present_chromium_marker"
    )]
    _chromium_marker: (),
}

fn parse_response(
    raw: &[u8],
    digest: &[u8; 32],
    rp: &str,
    origin: &str,
) -> RegistrationResult<RegistrationDescriptor> {
    if raw.is_empty() || raw.len() > MAX_RESPONSE {
        return Err(RegistrationError::Refused);
    }
    let response: StrictCredential =
        serde_json::from_slice(raw).map_err(|_| RegistrationError::Refused)?;
    let credential = decode(&response.raw_id, MAX_KEY)?;
    if credential.is_empty()
        || response.id != response.raw_id
        || response.kind != "public-key"
        || response.response.transports.len() > 6
        || response
            .attachment
            .as_deref()
            .is_some_and(|s| !matches!(s, "platform" | "cross-platform"))
    {
        return Err(RegistrationError::Refused);
    }
    for (index, transport) in response.response.transports.iter().enumerate() {
        if !matches!(
            transport.as_str(),
            "usb" | "nfc" | "ble" | "smart-card" | "hybrid" | "internal"
        ) || response.response.transports[..index].contains(transport)
        {
            return Err(RegistrationError::Refused);
        }
    }
    let client: StrictClientData =
        serde_json::from_slice(&decode(&response.response.client_data, MAX_COMPONENT)?)
            .map_err(|_| RegistrationError::Refused)?;
    if client.kind != "webauthn.create"
        || client.origin != origin
        || client.cross_origin
        || decode(&client.challenge, 32)? != digest
    {
        return Err(RegistrationError::Refused);
    }
    let attestation = canonical_cbor(&decode(&response.response.attestation, MAX_COMPONENT)?)?;
    let map = attestation.as_map().ok_or(RegistrationError::Refused)?;
    if map.len() != 3 {
        return Err(RegistrationError::Refused);
    }
    let mut fmt = None;
    let mut auth = None;
    let mut stmt = None;
    for (key, value) in map {
        match key.as_text() {
            Some("fmt") if fmt.is_none() => fmt = Some(value),
            Some("authData") if auth.is_none() => auth = Some(value),
            Some("attStmt") if stmt.is_none() => stmt = Some(value),
            _ => return Err(RegistrationError::Refused),
        }
    }
    if fmt.and_then(Value::as_text) != Some("none")
        || !stmt.and_then(Value::as_map).is_some_and(Vec::is_empty)
    {
        return Err(RegistrationError::Refused);
    }
    let auth = auth
        .and_then(Value::as_bytes)
        .ok_or(RegistrationError::Refused)?;
    if auth.len() < 55
        || auth[..32] != Sha256::digest(rp.as_bytes())[..]
        || auth[32] & 0xc7 != 0x45
        || auth[32] & 0x22 != 0
        || (auth[32] & 0x10 != 0 && auth[32] & 0x08 == 0)
    {
        return Err(RegistrationError::Refused);
    }
    let length = u16::from_be_bytes([auth[53], auth[54]]) as usize;
    if length == 0
        || length > MAX_KEY
        || auth.len() <= 55 + length
        || auth.len() - 55 - length > MAX_KEY
        || auth[55..55 + length] != credential
    {
        return Err(RegistrationError::Refused);
    }
    let public_key = auth[55 + length..].to_vec();
    let algorithm = validate_key(&public_key)?;
    Ok(RegistrationDescriptor {
        aaguid: auth[37..53]
            .try_into()
            .map_err(|_| RegistrationError::Refused)?,
        counter: u32::from_be_bytes(
            auth[33..37]
                .try_into()
                .map_err(|_| RegistrationError::Refused)?,
        ),
        backup_eligible: auth[32] & 8 != 0,
        backup_state: auth[32] & 16 != 0,
        algorithm,
        public_key,
        credential,
        transports: response.response.transports,
        attachment: response.attachment,
        opaque_user_handle: Vec::new(),
    })
}

fn canonical_cbor(raw: &[u8]) -> RegistrationResult<Value> {
    let mut cursor = Cursor::new(raw);
    let value: Value = ciborium::de::from_reader_with_recursion_limit(&mut cursor, 8)
        .map_err(|_| RegistrationError::Refused)?;
    let mut encoded = Vec::new();
    ciborium::into_writer(&value, &mut encoded).map_err(|_| RegistrationError::Refused)?;
    if cursor.position() != raw.len() as u64 || encoded != raw {
        return Err(RegistrationError::Refused);
    }
    Ok(value)
}
fn validate_key(raw: &[u8]) -> RegistrationResult<i32> {
    let value = canonical_cbor(raw)?;
    let map = value.as_map().ok_or(RegistrationError::Refused)?;
    let mut labels = std::collections::BTreeMap::new();
    for (key, value) in map {
        let key = key
            .as_integer()
            .and_then(|i| i64::try_from(i).ok())
            .ok_or(RegistrationError::Refused)?;
        if labels.insert(key, value).is_some() {
            return Err(RegistrationError::Refused);
        }
    }
    let integer = |key| {
        labels
            .get(&key)
            .and_then(|v| v.as_integer())
            .and_then(|i| i64::try_from(i).ok())
    };
    let bytes = |key| {
        labels
            .get(&key)
            .and_then(|v| v.as_bytes())
            .filter(|b| b.len() == 32)
    };
    match (integer(1), integer(3), integer(-1), labels.len()) {
        (Some(1), Some(-8), Some(6), 4) => {
            let public: [u8; 32] = bytes(-2)
                .ok_or(RegistrationError::Refused)?
                .as_slice()
                .try_into()
                .map_err(|_| RegistrationError::Refused)?;
            let key = ed25519_dalek::VerifyingKey::from_bytes(&public)
                .map_err(|_| RegistrationError::Refused)?;
            if key.is_weak() {
                return Err(RegistrationError::Refused);
            }
            Ok(-8)
        }
        (Some(2), Some(-7), Some(1), 5) => {
            let mut point = vec![4];
            point.extend_from_slice(bytes(-2).ok_or(RegistrationError::Refused)?);
            point.extend_from_slice(bytes(-3).ok_or(RegistrationError::Refused)?);
            graviola::signing::ecdsa::VerifyingKey::<graviola::signing::ecdsa::P256>::from_x962_uncompressed(&point)
                .map_err(|_| RegistrationError::Refused)?;
            Ok(-7)
        }
        _ => Err(RegistrationError::Refused),
    }
}

fn check_stored(strict: &RegistrationDescriptor, stored: &StoredPasskey) -> RegistrationResult<()> {
    let transports =
        serde_json::to_value(&stored.transports).map_err(|_| RegistrationError::Refused)?;
    if stored.credential_id != strict.credential
        || stored.public_key != strict.public_key
        || stored.algorithm != strict.algorithm
        || stored.counter != strict.counter
        || stored.aaguid != strict.aaguid
        || stored.be != strict.backup_eligible
        || stored.bs != strict.backup_state
        || stored.attestation_type != AttestationType::None
        || transports != serde_json::json!(strict.transports)
        || stored.rk.is_some()
        || stored.large_blob_supported.is_some()
        || stored.cred_protect.is_some()
        || stored.min_pin_length.is_some()
    {
        return Err(RegistrationError::Refused);
    }
    Ok(())
}
fn encode(raw: &[u8]) -> String {
    Base64UrlUnpadded::encode_string(raw)
}
fn decode(text: &str, limit: usize) -> RegistrationResult<Vec<u8>> {
    if text.len() > limit.div_ceil(3) * 4 {
        return Err(RegistrationError::Refused);
    }
    let raw = Base64UrlUnpadded::decode_vec(text).map_err(|_| RegistrationError::Refused)?;
    if raw.len() > limit || encode(&raw) != text {
        return Err(RegistrationError::Refused);
    }
    Ok(raw)
}
fn system_factory(
    passki: &Passki,
    user: &[u8],
) -> RegistrationResult<(RegistrationChallenge, RegistrationState)> {
    let mut options = RegistrationOptions::default();
    options.user_verification = UserVerificationRequirement::Required;
    options.resident_key = ResidentKeyRequirement::Discouraged;
    passki
        .start_passkey_registration(user, "opaque", "opaque", options)
        .map_err(|_| RegistrationError::Refused)
}
fn system_time() -> RegistrationResult<u64> {
    crate::enrollment_proof::current_time().map_err(|_| RegistrationError::Unavailable)
}
fn system_entropy(bytes: &mut [u8]) -> RegistrationResult<()> {
    getrandom::getrandom(bytes).map_err(|_| RegistrationError::Unavailable)
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
mod tests;

pub(crate) mod confirmation;

#[cfg(all(test, feature = "browser-lab", target_os = "linux"))]
mod browser_tests;
