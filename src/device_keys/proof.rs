use super::refused;
use crate::failure::Failure;
use ed25519_dalek::{Signature, VerifyingKey};
#[cfg(any(windows, test))]
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const DOMAIN: &[u8] = b"vivac.device.enrollment-proof.v1\0";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Challenge {
    pub(super) version: u64,
    pub(super) organization_id: String,
    pub(super) principal_id: String,
    pub(super) device_id: String,
    pub(super) credential_id: String,
    pub(super) public_key: String,
    pub(super) operation_id: String,
    pub(super) base_revision: u64,
    pub(super) nonce: String,
    pub(super) issued_at: u64,
    pub(super) expires_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Proof {
    pub(super) challenge: Challenge,
    pub(super) signature: String,
    pub(super) status: String,
    pub(super) authority: String,
    pub(super) challenge_use: String,
}

pub(super) fn current_time() -> Result<u64, Failure> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| refused("the operating-system clock is unavailable."))
}

impl Challenge {
    pub(super) fn validate(&self, now: u64) -> Result<(), Failure> {
        self.canonical_bytes()?;
        self.expires_at
            .checked_sub(self.issued_at)
            .filter(|window| *window > 0 && *window <= 300)
            .ok_or_else(|| refused("the challenge time window is invalid."))?;
        if now < self.issued_at || now >= self.expires_at {
            return Err(refused("the challenge is outside its time window."));
        }
        Ok(())
    }

    pub(super) fn canonical_bytes(&self) -> Result<Vec<u8>, Failure> {
        if self.version != 1 {
            return Err(refused("the challenge version is unsupported."));
        }
        let identifiers = [
            &self.organization_id,
            &self.principal_id,
            &self.device_id,
            &self.credential_id,
            &self.operation_id,
        ];
        for identifier in identifiers {
            if identifier.len() != 26
                || !matches!(identifier.as_bytes()[0], b'0'..=b'7')
                || !identifier
                    .bytes()
                    .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&byte))
            {
                return Err(refused("a challenge identifier is not canonical."));
            }
        }
        let public = decode_hex::<32>(&self.public_key)?;
        let key = VerifyingKey::from_bytes(&public)
            .map_err(|_| refused("the challenge public key is invalid."))?;
        if key.is_weak() {
            return Err(refused("the challenge public key is weak."));
        }
        let nonce = decode_hex::<32>(&self.nonce)?;
        let mut bytes = Vec::with_capacity(251);
        bytes.extend_from_slice(DOMAIN);
        for identifier in &identifiers[..4] {
            bytes.extend_from_slice(identifier.as_bytes());
        }
        bytes.extend_from_slice(&public);
        bytes.extend_from_slice(self.operation_id.as_bytes());
        bytes.extend_from_slice(&self.base_revision.to_be_bytes());
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&self.issued_at.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        Ok(bytes)
    }
}

impl Proof {
    pub(super) fn verify(&self, now: u64) -> Result<(), Failure> {
        if self.status != "possession_only"
            || self.authority != "not_checked"
            || self.challenge_use != "not_checked"
        {
            return Err(refused("the proof labels are invalid."));
        }
        self.challenge.validate(now)?;
        let bytes = self.challenge.canonical_bytes()?;
        let key = VerifyingKey::from_bytes(&decode_hex::<32>(&self.challenge.public_key)?)
            .map_err(|_| refused("the challenge public key is invalid."))?;
        let signature = Signature::from_bytes(&decode_hex::<64>(&self.signature)?);
        key.verify_strict(&bytes, &signature)
            .map_err(|_| refused("the possession proof signature is invalid."))
    }

    #[cfg(any(windows, test))]
    pub(super) fn signed(
        challenge: Challenge,
        signing: &SigningKey,
        now: u64,
    ) -> Result<Self, Failure> {
        challenge.validate(now)?;
        if decode_hex::<32>(&challenge.public_key)? != *signing.verifying_key().as_bytes() {
            return Err(refused("the challenge does not match the signing key."));
        }
        let signature = encode_hex(&signing.sign(&challenge.canonical_bytes()?).to_bytes());
        Ok(Self {
            challenge,
            signature,
            status: "possession_only".into(),
            authority: "not_checked".into(),
            challenge_use: "not_checked".into(),
        })
    }
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], Failure> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(refused("a proof hexadecimal field is not canonical."));
    }
    let digit = |byte: u8| {
        if byte.is_ascii_digit() {
            byte - b'0'
        } else {
            byte - b'a' + 10
        }
    };
    let mut decoded = [0; N];
    for (target, pair) in decoded.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
        *target = (digit(pair[0]) << 4) | digit(pair[1]);
    }
    Ok(decoded)
}

#[cfg(any(windows, test))]
fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signing() -> SigningKey {
        SigningKey::from_bytes(&[0x42; 32])
    }

    fn challenge() -> Challenge {
        Challenge {
            version: 1,
            organization_id: "00000000000000000000000001".into(),
            principal_id: "00000000000000000000000002".into(),
            device_id: "00000000000000000000000003".into(),
            credential_id: "00000000000000000000000004".into(),
            public_key: encode_hex(signing().verifying_key().as_bytes()),
            operation_id: "00000000000000000000000005".into(),
            base_revision: 0x0102030405060708,
            nonce: "ab".repeat(32),
            issued_at: 1000,
            expires_at: 1300,
        }
    }

    #[test]
    fn canonical_layout_has_independent_golden_bytes() {
        let challenge = challenge();
        let mut golden = b"vivac.device.enrollment-proof.v1\0\
00000000000000000000000001000000000000000000000000020000000000000000000000000300000000000000000000000004".to_vec();
        golden.extend_from_slice(&[
            0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e,
            0xab, 0x6c, 0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06,
            0x98, 0x81, 0xdb, 0x12,
        ]);
        golden.extend_from_slice(b"00000000000000000000000005");
        golden.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        golden.extend_from_slice(&[0xab; 32]);
        golden.extend_from_slice(&[0, 0, 0, 0, 0, 0, 3, 0xe8]);
        golden.extend_from_slice(&[0, 0, 0, 0, 0, 0, 5, 0x14]);
        assert_eq!(golden.len(), 251);
        assert_eq!(challenge.canonical_bytes().unwrap(), golden);
    }

    #[test]
    fn strict_time_window_edges() {
        let original = challenge();
        assert!(original.validate(1000).is_ok());
        assert!(original.validate(1299).is_ok());
        assert!(original.validate(999).is_err());
        assert!(original.validate(1300).is_err());
        for (issued_at, expires_at, now) in [
            (1000, 1000, 1000),
            (1001, 1000, 1000),
            (1000, 1301, 1000),
            (u64::MAX, 0, u64::MAX),
            (0, u64::MAX, 0),
        ] {
            let mut changed = original.clone();
            changed.issued_at = issued_at;
            changed.expires_at = expires_at;
            assert!(changed.validate(now).is_err());
        }
        let mut high = original;
        high.issued_at = u64::MAX - 300;
        high.expires_at = u64::MAX;
        assert!(high.validate(u64::MAX - 1).is_ok());
    }

    #[test]
    fn canonical_fields_refuse_noncanonical_input() {
        let original = serde_json::to_value(challenge()).unwrap();
        for field in [
            "organization_id",
            "principal_id",
            "device_id",
            "credential_id",
            "operation_id",
        ] {
            for invalid in [
                "",
                "0000000000000000000000000",
                "80000000000000000000000000",
                "0000000000000000000000000i",
                "0000000000000000000000000A",
                "000000000000000000000000é",
            ] {
                let mut changed = original.clone();
                changed[field] = invalid.into();
                let parsed: Challenge = serde_json::from_value(changed).unwrap();
                assert!(parsed.validate(1100).is_err(), "{field}");
            }
        }
        for field in ["public_key", "nonce"] {
            for invalid in [
                "".to_owned(),
                "ab".repeat(31),
                "AB".repeat(32),
                "é".repeat(32),
                "gg".repeat(32),
            ] {
                let mut changed = original.clone();
                changed[field] = invalid.into();
                assert!(serde_json::from_value::<Challenge>(changed)
                    .unwrap()
                    .validate(1100)
                    .is_err());
            }
        }
        let mut changed = challenge();
        changed.version = 2;
        assert!(changed.validate(1100).is_err());
    }

    #[test]
    fn signatures_bind_every_challenge_field_and_fixed_labels() {
        let proof = Proof::signed(challenge(), &signing(), 1100).unwrap();
        proof.verify(1100).unwrap();
        proof.verify(1100).unwrap();
        let original = serde_json::to_value(&proof).unwrap();
        for (field, value) in [
            ("version", serde_json::json!(2)),
            (
                "organization_id",
                serde_json::json!("00000000000000000000000006"),
            ),
            (
                "principal_id",
                serde_json::json!("00000000000000000000000006"),
            ),
            ("device_id", serde_json::json!("00000000000000000000000006")),
            (
                "credential_id",
                serde_json::json!("00000000000000000000000006"),
            ),
            (
                "operation_id",
                serde_json::json!("00000000000000000000000006"),
            ),
            (
                "public_key",
                serde_json::json!(encode_hex(
                    SigningKey::from_bytes(&[0x43; 32])
                        .verifying_key()
                        .as_bytes()
                )),
            ),
            ("base_revision", serde_json::json!(1)),
            ("nonce", serde_json::json!("ac".repeat(32))),
            ("issued_at", serde_json::json!(1001)),
            ("expires_at", serde_json::json!(1299)),
        ] {
            let mut changed = original.clone();
            changed["challenge"][field] = value;
            assert!(
                serde_json::from_value::<Proof>(changed)
                    .unwrap()
                    .verify(1100)
                    .is_err(),
                "{field}"
            );
        }
        for field in ["status", "authority", "challenge_use", "signature"] {
            let mut changed = original.clone();
            changed[field] = "invalid".into();
            assert!(serde_json::from_value::<Proof>(changed)
                .unwrap()
                .verify(1100)
                .is_err());
        }
        let mut changed = proof.clone();
        changed.signature = "00".repeat(64);
        assert!(changed.verify(1100).is_err());
        changed.signature = proof.signature.to_uppercase();
        assert!(changed.verify(1100).is_err());
        assert!(Proof::signed(challenge(), &SigningKey::from_bytes(&[1; 32]), 1100).is_err());
    }

    #[test]
    fn weak_keys_and_cross_domain_signatures_are_refused() {
        let mut proof = Proof::signed(challenge(), &signing(), 1100).unwrap();
        let bytes = proof.challenge.canonical_bytes().unwrap();
        let mut other = b"vivac.team.change.v1\0".to_vec();
        other.extend_from_slice(&bytes[DOMAIN.len()..]);
        proof.signature = encode_hex(&signing().sign(&other).to_bytes());
        assert!(proof.verify(1100).is_err());
        proof.challenge.public_key = format!("01{}", "00".repeat(31));
        assert!(proof.verify(1100).is_err());
        assert!(proof.challenge.validate(1100).is_err());
    }

    #[test]
    fn every_signed_byte_and_signature_bit_is_bound() {
        let mut proof = Proof::signed(challenge(), &signing(), 1100).unwrap();
        let bytes = proof.challenge.canonical_bytes().unwrap();
        for index in 0..bytes.len() {
            let mut changed = bytes.clone();
            changed[index] ^= 1;
            proof.signature = encode_hex(&signing().sign(&changed).to_bytes());
            assert!(proof.verify(1100).is_err(), "signed byte {index}");
        }
        let signature = signing().sign(&bytes).to_bytes();
        for index in 0..signature.len() {
            for mask in [1, 2, 4, 8, 16, 32, 64, 128] {
                let mut changed = signature;
                changed[index] ^= mask;
                proof.signature = encode_hex(&changed);
                assert!(
                    proof.verify(1100).is_err(),
                    "signature byte {index}, mask {mask}"
                );
            }
        }
    }

    #[test]
    fn rfc8032_empty_message_vector_is_independent() {
        let seed =
            decode_hex::<32>("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
                .unwrap();
        let signing = SigningKey::from_bytes(&seed);
        assert_eq!(
            encode_hex(signing.verifying_key().as_bytes()),
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
        let signature = decode_hex::<64>("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b").unwrap();
        assert_eq!(signing.sign(b"").to_bytes(), signature);
        signing
            .verifying_key()
            .verify_strict(b"", &Signature::from_bytes(&signature))
            .unwrap();
    }
}
