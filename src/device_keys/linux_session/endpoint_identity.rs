use super::super::linux_current_admission::CurrentAdmission;
use super::super::linux_endpoint_codec::{ChallengeFrame, ProofFrame, TranscriptData};
use super::super::linux_gate::{Error, RequestOutcome};
use super::super::linux_ipc::{registration::EndpointProofConnection, ConnectedPeerContext};
use super::{
    peer_credentials, peer_matches, require, take_string, EndpointProofAttempt, NativeSession,
    SigningIdentity, LIMIT,
};
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn stage_error(label: &'static str) -> impl FnOnce(io::Error) -> io::Error {
    move |error| io::Error::new(error.kind(), label)
}

fn before(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::from(io::ErrorKind::TimedOut))
    } else {
        Ok(())
    }
}

fn nonce_with(
    deadline: Instant,
    mut random: impl FnMut(&mut [u8]) -> io::Result<usize>,
) -> io::Result<[u8; 32]> {
    let mut nonce = [0; 32];
    let mut used = 0;
    while used < nonce.len() {
        before(deadline)?;
        match random(&mut nonce[used..]) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(count) => {
                require(count <= nonce.len() - used)?;
                used += count;
            }
            Err(error) if matches!(error.raw_os_error(), Some(libc::EINTR | libc::EAGAIN)) => {
                before(deadline)?;
                std::thread::sleep(
                    Duration::from_millis(1)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(error) => return Err(error),
        }
        before(deadline)?;
    }
    require(nonce != [0; 32])?;
    Ok(nonce)
}

fn fresh_nonce(deadline: Instant) -> io::Result<[u8; 32]> {
    nonce_with(deadline, |bytes| {
        // SAFETY: the remaining nonce buffer is writable for its bounded length.
        let count =
            unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), libc::GRND_NONBLOCK) };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    })
}

fn expected_key(public_key: &[u8; 32]) -> io::Result<()> {
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| invalid())?;
    require(!key.is_weak())
}

fn validate_signed_proof(
    proof: &ProofFrame,
    expected: &TranscriptData,
    public_key: &[u8; 32],
) -> io::Result<()> {
    require(
        proof.echo == expected.challenge
            && proof.generation == expected.generation
            && proof.connection_nonce == expected.connection_nonce,
    )?;
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| invalid())?;
    key.verify_strict(
        &expected.encode()?,
        &Signature::from_bytes(&proof.signature),
    )
    .map_err(|_| invalid())
}

fn identity_blob(blob: &[u8], public_key: &[u8; 32]) -> io::Result<()> {
    expected_key(public_key)?;
    let mut input = blob;
    require(take_string(&mut input)? == b"ssh-ed25519")?;
    require(take_string(&mut input)? == public_key && input.is_empty())
}

fn parse_identity(
    response: &[u8],
    public_key: [u8; 32],
    comment: &[u8],
    transcript: &[u8; 238],
) -> io::Result<SigningIdentity> {
    require((5..=LIMIT).contains(&response.len()) && response[0] == 12)?;
    require(u32::from_be_bytes(response[1..5].try_into().map_err(|_| invalid())?) == 1)?;
    let mut input = &response[5..];
    let blob = take_string(&mut input)?;
    let observed_comment = take_string(&mut input)?;
    require(input.is_empty() && observed_comment == comment)?;
    identity_blob(blob, &public_key)?;
    Ok(SigningIdentity {
        blob: blob.to_vec(),
        public_key,
        challenge: transcript.to_vec(),
        _comment: observed_comment.to_vec(),
    })
}

fn transcript_data(
    canonical: &[u8],
    store: [u64; 6],
    generation: [u8; 32],
    challenge: [u8; 32],
    connection_nonce: [u8; 32],
    identities: ([u32; 3], [u32; 3]),
) -> TranscriptData {
    let (local, peer) = identities;
    TranscriptData {
        record_digest: Sha256::digest(canonical).into(),
        store,
        generation,
        challenge,
        connection_nonce,
        verifier: (peer[0], peer[1], peer[2]),
        custodian: (local[0], local[1], local[2]),
    }
}

fn verify_received_proof(
    proof: &ProofFrame,
    challenge: [u8; 32],
    canonical: &[u8],
    store: [u64; 6],
    public_key: &[u8; 32],
    (verifier, custodian): ([u32; 3], [u32; 3]),
) -> io::Result<VerifiedEndpointPossession> {
    expected_key(public_key)?;
    let transcript = transcript_data(
        canonical,
        store,
        proof.generation,
        challenge,
        proof.connection_nonce,
        (custodian, verifier),
    );
    validate_signed_proof(proof, &transcript, public_key)?;
    Ok(VerifiedEndpointPossession {
        generation: transcript.generation,
    })
}

/// The verified generation at the instant of one local proof exchange.
pub struct VerifiedEndpointPossession {
    generation: [u8; 32],
}

impl VerifiedEndpointPossession {
    pub fn generation(&self) -> [u8; 32] {
        self.generation
    }
}

impl EndpointProofConnection {
    /// Verifies one proof using the independently selected resident admission.
    pub fn verify_endpoint_possession(
        &mut self,
        admission: &CurrentAdmission,
    ) -> io::Result<VerifiedEndpointPossession> {
        let result = (|| {
            let canonical = admission.canonical_record()?;
            let store = admission.store_identity()?;
            let public_key = admission.expected_public_key()?;
            expected_key(&public_key)?;
            let (challenge, proof, mut peer) = self.request_proof()?;
            let identities = peer.identities()?;
            let verified = verify_received_proof(
                &proof,
                challenge,
                &canonical,
                store,
                &public_key,
                identities,
            )?;
            require(admission.canonical_record()? == canonical)?;
            require(admission.store_identity()? == store)?;
            require(admission.expected_public_key()? == public_key)?;
            require(peer.identities()? == identities)?;
            before(self.deadline())?;
            Ok(verified)
        })();
        if result.is_err() {
            self.refuse();
        }
        result
    }
}

struct Context {
    attempt: EndpointProofAttempt,
    admission: CurrentAdmission,
    peer: ConnectedPeerContext,
    invalidated: bool,
}

impl Context {
    fn check_custody(&self) -> io::Result<()> {
        if self.invalidated {
            return Err(invalid());
        }
        let (_, store) = self.attempt.binding_values()?;
        require(self.admission.store_identity()? == store)?;
        self.attempt.check_live()
    }

    fn eligible_peer(&mut self) -> Result<bool, Error> {
        self.check_custody().map_err(|_| Error::LeaseInvalidated)?;
        let eligible = self.peer.validate().is_ok();
        self.check_custody().map_err(|_| Error::LeaseInvalidated)?;
        Ok(eligible)
    }

    fn check_live(&mut self) -> io::Result<()> {
        let result = (|| {
            self.check_custody()?;
            self.check_witnesses()
        })();
        if result.is_err() {
            self.invalidated = true;
        }
        result
    }

    fn check_witnesses(&mut self) -> io::Result<()> {
        self.peer.validate()?;
        self.attempt.check_live()
    }

    fn check_session(&mut self, session: &NativeSession) -> io::Result<()> {
        if self.invalidated {
            return Err(invalid());
        }
        let result = (|| {
            self.check_live()?;
            session.validate_endpoint_attempt(&self.attempt)?;
            self.check_witnesses()
        })();
        if result.is_err() {
            self.invalidated = true;
        }
        result
    }
}

pub struct EndpointNativeIdentity {
    context: Context,
    identity: SigningIdentity,
    transcript: TranscriptData,
    invalidated: bool,
}

impl EndpointNativeIdentity {
    pub(super) fn validate_session(&mut self, session: &NativeSession) -> io::Result<()> {
        if self.invalidated {
            return Err(invalid());
        }
        let result = (|| {
            self.check_live()?;
            session.validate_endpoint_attempt(&self.context.attempt)?;
            self.context.check_witnesses()
        })();
        if result.is_err() {
            self.invalidated = true;
        }
        result
    }

    pub fn check_live(&mut self) -> io::Result<()> {
        if self.invalidated {
            return Err(invalid());
        }
        let result = (|| {
            self.context.check_live()?;
            let (generation, store) = self.context.attempt.binding_values()?;
            let record = self.context.admission.endpoint_record()?;
            let current = transcript_data(
                &record.canonical,
                store,
                generation,
                self.transcript.challenge,
                self.transcript.connection_nonce,
                self.context.peer.identities()?,
            )
            .encode()?;
            require(current == self.transcript.encode()?)?;
            require(self.identity.challenge == current)?;
            require(self.identity.public_key == record.public_key)?;
            require(self.identity._comment == record.comment)?;
            identity_blob(&self.identity.blob, &self.identity.public_key)?;
            self.context.check_witnesses()
        })();
        if result.is_err() {
            self.invalidated = true;
        }
        result
    }

    pub(super) fn prepare(
        session: &NativeSession,
        attempt: EndpointProofAttempt,
        admission: CurrentAdmission,
        peer: ConnectedPeerContext,
        challenge_bytes: &[u8],
    ) -> io::Result<Self> {
        let challenge = ChallengeFrame::decode(challenge_bytes)?;
        let mut context = Context {
            attempt,
            admission,
            peer,
            invalidated: false,
        };
        context
            .check_session(session)
            .map_err(stage_error("prepare-context"))?;
        let (generation, store) = context
            .attempt
            .binding_values()
            .map_err(stage_error("prepare-context"))?;
        let deadline = context
            .attempt
            .deadline()
            .map_err(stage_error("prepare-context"))?;
        let nonce = fresh_nonce(deadline).map_err(stage_error("prepare-nonce"))?;
        context
            .check_session(session)
            .map_err(stage_error("prepare-context"))?;
        let record = context.admission.endpoint_record()?;
        let transcript = transcript_data(
            &record.canonical,
            store,
            generation,
            challenge.nonce,
            nonce,
            context.peer.identities()?,
        );
        let message = transcript.encode()?;
        expected_key(&record.public_key)?;
        context
            .check_session(session)
            .map_err(stage_error("prepare-context"))?;
        let resources = session.resources.as_ref().ok_or_else(invalid)?;
        let io_deadline = deadline.min(
            Instant::now()
                .checked_add(Duration::from_secs(3))
                .ok_or_else(invalid)?,
        );
        let mut check = || context.check_session(session);
        let mut connection = connect(&resources.socket, io_deadline, &mut check)
            .map_err(stage_error("prepare-agent-connect"))?;
        check().map_err(stage_error("prepare-agent-peer"))?;
        require(peer_matches(resources.peer, peer_credentials(&connection)?))
            .map_err(stage_error("prepare-agent-peer"))?;
        check().map_err(stage_error("prepare-agent-peer"))?;
        let response = exchange(&mut connection, io_deadline, &mut check)?;
        check().map_err(stage_error("prepare-agent-peer"))?;
        require(peer_matches(resources.peer, peer_credentials(&connection)?))
            .map_err(stage_error("prepare-agent-peer"))?;
        check().map_err(stage_error("prepare-agent-peer"))?;
        before(io_deadline).map_err(stage_error("prepare-agent-deadline"))?;
        let identity = parse_identity(&response, record.public_key, &record.comment, &message)
            .map_err(stage_error("prepare-agent-identity"))?;
        context
            .check_session(session)
            .map_err(stage_error("prepare-revalidate"))?;
        let mut prepared = Self {
            context,
            identity,
            transcript,
            invalidated: false,
        };
        session
            .validate_endpoint_identity(&mut prepared)
            .map_err(stage_error("prepare-revalidate"))?;
        before(io_deadline).map_err(stage_error("prepare-final-deadline"))?;
        Ok(prepared)
    }
}

struct SignedEndpointFrame {
    prepared: EndpointNativeIdentity,
    wire: [u8; 176],
    id: u64,
    epoch: u64,
}

impl NativeSession {
    /// Authenticates and answers one challenge on the retained local connection.
    pub fn serve_endpoint_proof(
        &self,
        connection: &mut EndpointProofConnection,
        admission: CurrentAdmission,
    ) -> io::Result<()> {
        self.serve_endpoint_proof_checked(connection, admission, || Ok(()))
    }

    pub(crate) fn serve_endpoint_proof_checked(
        &self,
        connection: &mut EndpointProofConnection,
        admission: CurrentAdmission,
        before_send: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let result = (|| {
            let (challenge, peer) = connection
                .receive_challenge()
                .map_err(stage_error("receive-challenge"))?;
            let mut attempt = self
                .endpoint_attempt(connection.deadline())
                .map_err(stage_error("reserve-attempt"))?;
            self.consume_endpoint_attempt(&mut attempt)
                .map_err(stage_error("consume-attempt"))?;
            let prepared =
                EndpointNativeIdentity::prepare(self, attempt, admission, peer, &challenge)?;
            let mut signed = self
                .sign_endpoint_identity(prepared)
                .map_err(stage_error("sign-proof"))?;
            self.validate_endpoint_identity(&mut signed.prepared)
                .map_err(stage_error("validate-before-send"))?;
            before_send()?;
            self.gate()
                .publish_endpoint_frame(signed.id, signed.epoch, || {
                    connection.send_proof(&signed.wire)
                })
                .map_err(stage_error("send-proof"))?;
            self.validate_endpoint_identity(&mut signed.prepared)
                .map_err(stage_error("validate-after-send"))?;
            self.endpoint_proof_sent.set(true);
            Ok(())
        })();
        if result.is_err() {
            connection.refuse();
        }
        result
    }

    fn sign_endpoint_identity(
        &self,
        mut prepared: EndpointNativeIdentity,
    ) -> io::Result<SignedEndpointFrame> {
        self.validate_endpoint_identity(&mut prepared)?;
        let deadline = prepared.context.attempt.deadline()?;
        let gate = self.gate();
        let epoch = gate.snapshot().map_err(|_| invalid())?.epoch;
        let signer = self.signer(SigningIdentity {
            blob: prepared.identity.blob.clone(),
            challenge: prepared.identity.challenge.clone(),
            public_key: prepared.identity.public_key,
            _comment: prepared.identity._comment.clone(),
        })?;
        let context = Arc::new(Mutex::new(prepared));
        let worker_context = context.clone();
        let id = gate
            .start_endpoint(move |request| {
                let eligible = || {
                    worker_context
                        .lock()
                        .map_err(|_| Error::Poisoned)?
                        .context
                        .eligible_peer()
                };
                if !eligible()? {
                    return Ok(None);
                }
                // Once the native exchange starts, peer loss alone cannot
                // interrupt it. Drain and verify before abandoning its result.
                let signature = signer.sign(request, |_| {
                    worker_context
                        .lock()
                        .map_err(|_| Error::Poisoned)?
                        .context
                        .check_custody()
                        .map_err(|_| Error::LeaseInvalidated)
                })?;
                Ok(eligible()?.then_some(signature))
            })
            .map_err(|_| invalid())?;
        let outcome = gate
            .wait_request(id, deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                Error::Timeout => io::Error::from(io::ErrorKind::TimedOut),
                _ => invalid(),
            })?;
        require(outcome == RequestOutcome::Published)?;
        // Consume the stored proof even if the peer disappears before the
        // final checks. A refused connection cannot retain the gate's slot.
        let signature = gate
            .take_ready_proof(id, epoch)
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
        let mut prepared = Arc::try_unwrap(context)
            .map_err(|_| invalid())?
            .into_inner()
            .map_err(|_| invalid())?;
        self.validate_endpoint_identity(&mut prepared)?;
        let proof = ProofFrame {
            echo: prepared.transcript.challenge,
            generation: prepared.transcript.generation,
            connection_nonce: prepared.transcript.connection_nonce,
            signature,
        };
        validate_signed_proof(&proof, &prepared.transcript, &prepared.identity.public_key)?;
        let frame = proof.encode()?;
        self.validate_endpoint_identity(&mut prepared)?;
        Ok(SignedEndpointFrame {
            prepared,
            wire: frame,
            id,
            epoch,
        })
    }
}

fn wait(
    socket: &UnixStream,
    events: i16,
    deadline: Instant,
    check: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    loop {
        check()?;
        before(deadline)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let millis = remaining
            .as_nanos()
            .div_ceil(1_000_000)
            .min(i32::MAX as u128) as i32;
        let mut poll = libc::pollfd {
            fd: socket.as_raw_fd(),
            events,
            revents: 0,
        };
        // SAFETY: poll points to one initialized descriptor for the stated count.
        let result = unsafe { libc::poll(&mut poll, 1, millis.max(1)) };
        let error = if result < 0 {
            Some(io::Error::last_os_error())
        } else {
            None
        };
        check()?;
        before(deadline)?;
        if let Some(error) = error {
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if result > 0 {
            require(poll.revents & libc::POLLNVAL == 0)?;
            return Ok(());
        }
    }
}

fn connect(
    path: &Path,
    deadline: Instant,
    check: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<UnixStream> {
    check()?;
    before(deadline)?;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: the zeroed sockaddr_un is initialized before being passed to connect.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    require(path.is_absolute() && !bytes.contains(&0) && bytes.len() < address.sun_path.len())?;
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = *byte as libc::c_char;
    }
    // SAFETY: socket returns a new descriptor on success with the requested flags.
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful socket transferred one fresh descriptor.
    let socket = UnixStream::from(unsafe { OwnedFd::from_raw_fd(raw) });
    check()?;
    before(deadline)?;
    let size = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    // SAFETY: address is initialized and size covers its family and terminated path.
    let result = unsafe {
        libc::connect(
            socket.as_raw_fd(),
            std::ptr::addr_of!(address).cast(),
            size as libc::socklen_t,
        )
    };
    let error = if result < 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    check()?;
    before(deadline)?;
    if let Some(error) = error {
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        wait(&socket, libc::POLLOUT, deadline, check)?;
        let mut status: libc::c_int = 0;
        let mut length = std::mem::size_of_val(&status) as libc::socklen_t;
        check()?;
        before(deadline)?;
        // SAFETY: getsockopt writes only the initialized integer and its length.
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                std::ptr::addr_of_mut!(status).cast(),
                &mut length,
            )
        };
        let error = if result < 0 {
            Some(io::Error::last_os_error())
        } else {
            None
        };
        check()?;
        before(deadline)?;
        if let Some(error) = error {
            return Err(error);
        }
        require(length as usize == std::mem::size_of_val(&status))?;
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status));
        }
    }
    Ok(socket)
}

fn transfer(
    socket: &mut UnixStream,
    buffer: &mut [u8],
    writing: bool,
    deadline: Instant,
    check: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let mut used = 0;
    while used < buffer.len() {
        check()?;
        before(deadline)?;
        let result = if writing {
            socket.write(&buffer[used..])
        } else {
            socket.read(&mut buffer[used..])
        };
        check()?;
        before(deadline)?;
        match result {
            Ok(0) => {
                return Err(io::Error::from(if writing {
                    io::ErrorKind::WriteZero
                } else {
                    io::ErrorKind::UnexpectedEof
                }))
            }
            Ok(size) => used += size,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait(
                    socket,
                    if writing { libc::POLLOUT } else { libc::POLLIN },
                    deadline,
                    check,
                )?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn exchange(
    socket: &mut UnixStream,
    deadline: Instant,
    check: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    transfer(socket, &mut [0, 0, 0, 1, 11], true, deadline, check)
        .map_err(stage_error("prepare-agent-write"))?;
    let mut length = [0; 4];
    transfer(socket, &mut length, false, deadline, check)
        .map_err(stage_error("prepare-agent-header"))?;
    let size = u32::from_be_bytes(length) as usize;
    require((1..=LIMIT).contains(&size))?;
    let mut response = vec![0; size];
    transfer(socket, &mut response, false, deadline, check)
        .map_err(stage_error("prepare-agent-body"))?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::super::ssh_string;
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key(seed: u8) -> [u8; 32] {
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes()
    }
    fn transcript() -> TranscriptData {
        transcript_data(
            b"public canonical record",
            [1, 2, 3, 4, 5, 6],
            [2; 32],
            [3; 32],
            [4; 32],
            ([202, 1000, 1000], [101, 1000, 1000]),
        )
    }

    #[test]
    fn verified_endpoint_signature_cannot_be_published_in_a_successor_generation() {
        let signer = SigningKey::from_bytes(&[9; 32]);
        let public_key = signer.verifying_key().to_bytes();
        let previous = transcript();
        let mut proof = ProofFrame {
            echo: previous.challenge,
            generation: previous.generation,
            connection_nonce: previous.connection_nonce,
            signature: signer.sign(&previous.encode().unwrap()).to_bytes(),
        };
        let wire = proof.encode().unwrap();
        let decoded = ProofFrame::decode(&wire).unwrap();
        validate_signed_proof(&decoded, &previous, &public_key).unwrap();

        let successor = TranscriptData {
            generation: [5; 32],
            ..previous
        };
        assert!(validate_signed_proof(&decoded, &successor, &public_key).is_err());
        proof.generation = successor.generation;
        assert!(validate_signed_proof(&proof, &successor, &public_key).is_err());
        proof.signature = signer.sign(&successor.encode().unwrap()).to_bytes();
        validate_signed_proof(&proof, &successor, &public_key).unwrap();
    }

    #[test]
    fn verifier_reconstructs_signed_bytes_from_its_own_context() {
        let signer = SigningKey::from_bytes(&[9; 32]);
        let key = signer.verifying_key().to_bytes();
        let old = transcript();
        let record = b"public canonical record";
        let verifier = [101, 1000, 1000];
        let custodian = [202, 1000, 1000];
        let mut proof = ProofFrame {
            echo: old.challenge,
            generation: old.generation,
            connection_nonce: old.connection_nonce,
            signature: signer.sign(&old.encode().unwrap()).to_bytes(),
        };
        let verified = verify_received_proof(
            &proof,
            old.challenge,
            record,
            old.store,
            &key,
            (verifier, custodian),
        )
        .unwrap();
        assert_eq!(verified.generation(), old.generation);
        for (challenge, record, store, key, peers) in [
            (
                [5; 32],
                record.as_slice(),
                old.store,
                key,
                (verifier, custodian),
            ),
            (
                old.challenge,
                b"other record".as_slice(),
                old.store,
                key,
                (verifier, custodian),
            ),
            (
                old.challenge,
                record.as_slice(),
                [9; 6],
                key,
                (verifier, custodian),
            ),
            (
                old.challenge,
                record.as_slice(),
                old.store,
                SigningKey::from_bytes(&[10; 32]).verifying_key().to_bytes(),
                (verifier, custodian),
            ),
            (
                old.challenge,
                record.as_slice(),
                old.store,
                key,
                (custodian, verifier),
            ),
        ] {
            assert!(verify_received_proof(&proof, challenge, record, store, &key, peers).is_err());
        }
        proof.connection_nonce[0] ^= 1;
        assert!(verify_received_proof(
            &proof,
            old.challenge,
            record,
            old.store,
            &key,
            (verifier, custodian)
        )
        .is_err());
    }
    fn reply(public_key: &[u8], comment: &[u8], algorithm: &[u8], extra_blob: bool) -> Vec<u8> {
        let mut blob = Vec::new();
        ssh_string(algorithm, &mut blob).unwrap();
        ssh_string(public_key, &mut blob).unwrap();
        if extra_blob {
            blob.push(0);
        }
        let mut response = vec![12, 0, 0, 0, 1];
        ssh_string(&blob, &mut response).unwrap();
        ssh_string(comment, &mut response).unwrap();
        response
    }

    #[test]
    fn endpoint_identity_parser_binds_exact_key_comment_and_private_transcript() {
        let public_key = key(9);
        let message = transcript().encode().unwrap();
        let response = reply(
            &public_key,
            b"exact protected comment",
            b"ssh-ed25519",
            false,
        );
        let identity =
            parse_identity(&response, public_key, b"exact protected comment", &message).unwrap();
        assert_eq!(identity.public_key, public_key);
        assert_eq!(identity._comment, b"exact protected comment");
        assert_eq!(identity.challenge, message);
        assert!(identity
            .challenge
            .starts_with(b"vivac.linux.endpoint-proof.v1\0"));
    }

    #[test]
    fn endpoint_identity_parser_refuses_mismatch_count_extra_algorithm_bounds_and_weak_keys() {
        let public_key = key(9);
        let message = transcript().encode().unwrap();
        let valid = reply(&public_key, b"protected", b"ssh-ed25519", false);
        let mut weak = [0; 32];
        weak[0] = 1;
        assert!(parse_identity(
            &reply(&weak, b"protected", b"ssh-ed25519", false),
            weak,
            b"protected",
            &message
        )
        .is_err());
        let mut cases = vec![
            reply(&key(10), b"protected", b"ssh-ed25519", false),
            reply(&public_key, b"wrong comment", b"ssh-ed25519", false),
            reply(&public_key, b"protected", b"ssh-rsa", false),
            reply(&public_key[..31], b"protected", b"ssh-ed25519", false),
            reply(&public_key, b"protected", b"ssh-ed25519", true),
            vec![0; LIMIT + 1],
        ];
        for count in [0, 2, u32::MAX] {
            let mut changed = valid.clone();
            changed[1..5].copy_from_slice(&count.to_be_bytes());
            cases.push(changed);
        }
        let mut changed = valid.clone();
        changed.push(0);
        cases.push(changed);
        let mut changed = valid.clone();
        changed[0] = 14;
        cases.push(changed);
        let mut changed = valid.clone();
        changed[5..9].copy_from_slice(&u32::MAX.to_be_bytes());
        cases.push(changed);
        for changed in cases {
            assert!(parse_identity(&changed, public_key, b"protected", &message).is_err());
        }
        for end in 0..valid.len() {
            assert!(parse_identity(&valid[..end], public_key, b"protected", &message).is_err());
        }
    }

    #[test]
    fn endpoint_identity_transcript_derives_digest_store_roles_and_all_fixed_offsets() {
        let message = transcript().encode().unwrap();
        assert_eq!(message.len(), 238);
        assert_eq!(&message[..30], b"vivac.linux.endpoint-proof.v1\0");
        assert_eq!(&message[30..38], &[1, 2, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            &message[38..70],
            &Sha256::digest(b"public canonical record")[..]
        );
        for (index, value) in [1u64, 2, 3, 4, 5, 6].iter().enumerate() {
            assert_eq!(
                &message[70 + index * 8..78 + index * 8],
                &value.to_be_bytes()
            );
        }
        assert_eq!(&message[118..150], &[2; 32]);
        assert_eq!(&message[150..182], &[3; 32]);
        assert_eq!(&message[182..214], &[4; 32]);
        for (index, value) in [101u32, 1000, 1000, 202, 1000, 1000].iter().enumerate() {
            assert_eq!(
                &message[214 + index * 4..218 + index * 4],
                &value.to_be_bytes()
            );
        }
    }

    #[test]
    fn endpoint_identity_challenge_and_nonce_refuse_zero_expiry_and_native_failure() {
        let mut challenge = ChallengeFrame { nonce: [1; 32] }.encode().unwrap();
        assert!(ChallengeFrame::decode(&challenge).is_ok());
        challenge[16..].fill(0);
        assert!(ChallengeFrame::decode(&challenge).is_err());
        let future = Instant::now() + Duration::from_secs(1);
        assert!(nonce_with(future, |bytes| {
            bytes.fill(0);
            Ok(bytes.len())
        })
        .is_err());
        assert_eq!(
            nonce_with(future, |_| Err(io::Error::from_raw_os_error(libc::EIO)))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EIO)
        );
        let called = std::cell::Cell::new(false);
        let expired = nonce_with(Instant::now(), |_| {
            called.set(true);
            Ok(0)
        });
        assert_eq!(expired.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(!called.get());
        let nonce = nonce_with(future, |bytes| {
            let count = bytes.len().min(3);
            bytes[..count].fill(9);
            Ok(count)
        })
        .unwrap();
        assert_eq!(nonce, [9; 32]);
    }

    #[test]
    fn endpoint_identity_exchange_handles_partial_frames_with_checks_between_chunks() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            let mut request = [0; 5];
            server.read_exact(&mut request).unwrap();
            assert_eq!(request, [0, 0, 0, 1, 11]);
            for byte in [0, 0, 0, 6, 12, 0, 0, 0, 1, 88] {
                server.write_all(&[byte]).unwrap();
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let mut checks = 0;
        let response = exchange(
            &mut client,
            Instant::now() + Duration::from_secs(2),
            &mut || {
                checks += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(response, [12, 0, 0, 0, 1, 88]);
        assert!(checks >= 6);
        worker.join().unwrap();
    }

    #[test]
    fn endpoint_identity_exchange_refuses_eof_oversized_frame_and_expired_shared_deadline() {
        for oversized in [true, false] {
            let (mut client, mut server) = UnixStream::pair().unwrap();
            client.set_nonblocking(true).unwrap();
            let worker = std::thread::spawn(move || {
                let mut request = [0; 5];
                server.read_exact(&mut request).unwrap();
                if oversized {
                    server
                        .write_all(&((LIMIT + 1) as u32).to_be_bytes())
                        .unwrap();
                } else {
                    server.write_all(&[0, 0]).unwrap();
                }
            });
            let error = exchange(
                &mut client,
                Instant::now() + Duration::from_secs(1),
                &mut || Ok(()),
            )
            .unwrap_err();
            if !oversized {
                assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
            }
            worker.join().unwrap();
        }
        let (mut client, _server) = UnixStream::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        let error = exchange(
            &mut client,
            Instant::now() + Duration::from_millis(20),
            &mut || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
