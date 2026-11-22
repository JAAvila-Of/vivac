use super::linux_creation_record::Record;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use std::io;
use std::time::{Duration, Instant};

const DOMAIN: &[u8] = b"vivac.local.creation-binding.v1\0";
const LIFETIME: Duration = Duration::from_secs(30);

/// A one-attempt possession check, not creation provenance or admission.
pub struct LocalCreationChallenge {
    record_digest: [u8; 32],
    public_key: [u8; 32],
    comment: Vec<u8>,
    message: Vec<u8>,
    started: Instant,
}

/// Verified possession for one record, not authority to publish that record.
pub struct VerifiedCreationBinding {
    record_digest: [u8; 32],
}

struct PreparedRecord {
    digest: [u8; 32],
    public_key: [u8; 32],
    comment: Vec<u8>,
}

fn refused() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

fn before_deadline(started: Instant, now: Instant) -> io::Result<()> {
    let elapsed = now.checked_duration_since(started).ok_or_else(refused)?;
    if elapsed >= LIFETIME {
        return Err(refused());
    }
    Ok(())
}

impl LocalCreationChallenge {
    pub fn issue(record: &Record) -> io::Result<Self> {
        let prepared = PreparedRecord::new(record)?;
        let mut nonce = [0; 32];
        // SAFETY: nonce is writable for the exact requested length. No fallback
        // or retry accepts a partial, interrupted or unavailable random read.
        let received =
            unsafe { libc::getrandom(nonce.as_mut_ptr().cast(), nonce.len(), libc::GRND_NONBLOCK) };
        if received != nonce.len() as libc::ssize_t {
            return Err(refused());
        }
        let message = message(&prepared.digest, &nonce);
        Ok(Self {
            record_digest: prepared.digest,
            public_key: prepared.public_key,
            comment: prepared.comment,
            message,
            started: Instant::now(),
        })
    }

    #[cfg(test)]
    fn issue_with(
        record: &Record,
        started: Instant,
        random: impl FnOnce(&mut [u8; 32]) -> io::Result<()>,
    ) -> io::Result<Self> {
        let prepared = PreparedRecord::new(record)?;
        let mut nonce = [0; 32];
        random(&mut nonce)?;
        Ok(Self {
            record_digest: prepared.digest,
            public_key: prepared.public_key,
            comment: prepared.comment,
            message: message(&prepared.digest, &nonce),
            started,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.message
    }

    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    pub fn comment(&self) -> &[u8] {
        &self.comment
    }

    pub fn check_live(&self) -> io::Result<()> {
        before_deadline(self.started, Instant::now())
    }

    pub fn verify(self, signature: &[u8; 64]) -> io::Result<VerifiedCreationBinding> {
        let started = self.started;
        let binding = self.verify_at(signature, Instant::now())?;
        before_deadline(started, Instant::now())?;
        Ok(binding)
    }

    fn verify_at(self, signature: &[u8; 64], now: Instant) -> io::Result<VerifiedCreationBinding> {
        before_deadline(self.started, now)?;
        VerifyingKey::from_bytes(&self.public_key)
            .map_err(|_| refused())?
            .verify_strict(&self.message, &Signature::from_bytes(signature))
            .map_err(|_| refused())?;
        Ok(VerifiedCreationBinding {
            record_digest: self.record_digest,
        })
    }
}

impl VerifiedCreationBinding {
    pub fn matches_record(&self, record: &Record) -> io::Result<bool> {
        let digest: [u8; 32] = Sha256::digest(record.canonical_bytes()?).into();
        Ok(self.record_digest == digest)
    }
}

impl PreparedRecord {
    fn new(record: &Record) -> io::Result<Self> {
        let digest = Sha256::digest(record.canonical_bytes()?).into();
        let mut public_key = [0; 32];
        // Canonical validation above requires exactly 64 lowercase hex bytes.
        for (output, pair) in public_key
            .iter_mut()
            .zip(record.public_key.as_bytes().as_chunks::<2>().0.iter())
        {
            let digit = |byte: u8| {
                if byte.is_ascii_digit() {
                    byte - b'0'
                } else {
                    byte - b'a' + 10
                }
            };
            *output = digit(pair[0]) * 16 + digit(pair[1]);
        }
        let comment = format!(
            "vivac.device-key.v1:{}:{}",
            record.device_id, record.credential_id
        )
        .into_bytes();
        Ok(Self {
            digest,
            public_key,
            comment,
        })
    }
}

fn message(record_digest: &[u8; 32], nonce: &[u8; 32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(DOMAIN.len() + 64);
    bytes.extend_from_slice(DOMAIN);
    bytes.extend_from_slice(record_digest);
    bytes.extend_from_slice(nonce);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use std::cell::Cell;
    use std::time::Duration;

    const ENROLLMENT_DOMAIN: &[u8] = b"vivac.device.enrollment-proof.v1\0";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn fixture() -> (Record, SigningKey) {
        let key = SigningKey::from_bytes(&[7; 32]);
        let record = Record {
            version: 2,
            installation_id: "00000000000000000000000001".to_owned(),
            creation_id: "00000000000000000000000002".to_owned(),
            device_id: "00000000000000000000000003".to_owned(),
            credential_id: "00000000000000000000000004".to_owned(),
            public_key: hex(key.verifying_key().as_bytes()),
            ciphertext_sha256: "11".repeat(32),
            manifest_sha256: "22".repeat(32),
            ciphertext_len: 128,
            manifest_len: 256,
            creation_profile_revision: 1,
            secret_policy_revision: 1,
        };
        (record, key)
    }

    fn issue(record: &Record, started: Instant, nonce: [u8; 32]) -> LocalCreationChallenge {
        LocalCreationChallenge::issue_with(record, started, |output| {
            *output = nonce;
            Ok(())
        })
        .unwrap()
    }

    #[test]
    fn verifies_native_possession_for_the_exact_canonical_record() {
        let (record, key) = fixture();
        let started = Instant::now();
        let challenge = issue(&record, started, [9; 32]);
        let digest = Sha256::digest(record.canonical_bytes().unwrap());
        let mut expected = DOMAIN.to_vec();
        expected.extend_from_slice(&digest);
        expected.extend_from_slice(&[9; 32]);
        assert_eq!(challenge.bytes(), expected);
        assert_eq!(challenge.public_key(), key.verifying_key().as_bytes());
        assert_eq!(
            challenge.comment(),
            b"vivac.device-key.v1:00000000000000000000000003:00000000000000000000000004"
        );
        let signature = key.sign(challenge.bytes()).to_bytes();
        let binding = challenge
            .verify_at(&signature, started + Duration::from_secs(1))
            .unwrap();
        assert!(binding.matches_record(&record).unwrap());
    }

    #[test]
    fn every_supported_record_field_is_bound_to_the_signature_and_result() {
        let (record, key) = fixture();
        let started = Instant::now();
        let challenge = issue(&record, started, [9; 32]);
        let signature = key.sign(challenge.bytes()).to_bytes();
        let binding = challenge.verify_at(&signature, started).unwrap();
        let other_key = SigningKey::from_bytes(&[8; 32]);
        let mut variants = Vec::new();
        for field in 0..9 {
            let mut changed = record.clone();
            match field {
                0 => changed.installation_id = "00000000000000000000000005".to_owned(),
                1 => changed.creation_id = "00000000000000000000000005".to_owned(),
                2 => changed.device_id = "00000000000000000000000005".to_owned(),
                3 => changed.credential_id = "00000000000000000000000005".to_owned(),
                4 => changed.public_key = hex(other_key.verifying_key().as_bytes()),
                5 => changed.ciphertext_sha256 = "33".repeat(32),
                6 => changed.manifest_sha256 = "33".repeat(32),
                7 => changed.ciphertext_len += 1,
                _ => changed.manifest_len += 1,
            }
            variants.push(changed);
        }
        for changed in variants {
            assert!(changed.canonical_bytes().is_ok());
            assert!(!binding.matches_record(&changed).unwrap());
            assert!(issue(&changed, started, [9; 32])
                .verify_at(&signature, started)
                .is_err());
        }
    }

    #[test]
    fn rejects_other_purposes_nonces_keys_and_modified_signatures() {
        let (record, key) = fixture();
        let started = Instant::now();
        let challenge = issue(&record, started, [9; 32]);
        let signature = key.sign(challenge.bytes()).to_bytes();
        assert!(issue(&record, started, [10; 32])
            .verify_at(&signature, started)
            .is_err());
        let mut enrollment = ENROLLMENT_DOMAIN.to_vec();
        enrollment.extend_from_slice(&challenge.bytes()[DOMAIN.len()..]);
        let signature = key.sign(&enrollment).to_bytes();
        assert!(challenge.verify_at(&signature, started).is_err());
        let challenge = issue(&record, started, [9; 32]);
        let wrong_key = SigningKey::from_bytes(&[8; 32]);
        let signature = wrong_key.sign(challenge.bytes()).to_bytes();
        assert!(challenge.verify_at(&signature, started).is_err());
        let challenge = issue(&record, started, [9; 32]);
        let mut signature = key.sign(challenge.bytes()).to_bytes();
        signature[63] ^= 1;
        assert!(challenge.verify_at(&signature, started).is_err());
    }

    #[test]
    fn monotonic_deadline_is_not_extended_and_rejects_a_reversed_clock() {
        let (record, key) = fixture();
        let started = Instant::now();
        let challenge = issue(&record, started, [9; 32]);
        let signature = key.sign(challenge.bytes()).to_bytes();
        assert!(challenge
            .verify_at(&signature, started + Duration::from_millis(29999))
            .is_ok());
        for elapsed in [Duration::from_secs(30), Duration::from_secs(31)] {
            let challenge = issue(&record, started, [9; 32]);
            let signature = key.sign(challenge.bytes()).to_bytes();
            assert!(challenge.verify_at(&signature, started + elapsed).is_err());
        }
        let later = started + Duration::from_secs(1);
        let challenge = issue(&record, later, [9; 32]);
        let signature = key.sign(challenge.bytes()).to_bytes();
        assert!(challenge.verify_at(&signature, started).is_err());
    }

    #[test]
    fn invalid_record_revisions_and_shapes_refuse_before_random_generation() {
        let (record, _) = fixture();
        let mut variants = Vec::new();
        for field in 0..5 {
            let mut changed = record.clone();
            match field {
                0 => changed.version = 1,
                1 => changed.creation_profile_revision = 2,
                2 => changed.secret_policy_revision = 2,
                3 => changed.public_key = "00".repeat(32),
                _ => changed.ciphertext_len = 65537,
            }
            variants.push(changed);
        }
        for changed in variants {
            let called = Cell::new(false);
            assert!(
                LocalCreationChallenge::issue_with(&changed, Instant::now(), |_| {
                    called.set(true);
                    Ok(())
                })
                .is_err()
            );
            assert!(!called.get());
        }
    }

    #[test]
    fn random_failure_cannot_issue_a_challenge() {
        let (record, _) = fixture();
        let called = Cell::new(false);
        assert!(
            LocalCreationChallenge::issue_with(&record, Instant::now(), |_| {
                called.set(true);
                Err(io::Error::from(io::ErrorKind::Other))
            })
            .is_err()
        );
        assert!(called.get());
    }

    #[test]
    fn production_issuance_uses_fresh_nonces_and_verifies_without_test_clock() {
        let (record, key) = fixture();
        let first = LocalCreationChallenge::issue(&record).unwrap();
        let second = LocalCreationChallenge::issue(&record).unwrap();
        assert_ne!(first.bytes(), second.bytes());
        assert!(first.check_live().is_ok());
        let signature = key.sign(first.bytes()).to_bytes();
        assert!(first
            .verify(&signature)
            .unwrap()
            .matches_record(&record)
            .unwrap());
        assert!(second.verify(&signature).is_err());
    }
}
