use super::proof::{Challenge, Proof};
use super::{refused, PublicDeviceKey};
use crate::failure::Failure;
use ed25519_dalek::SigningKey;

pub(super) const PAYLOAD: &[u8] = b"vivac.device-key.v1\0";
pub(super) const PAYLOAD_SIZE: usize = 84;

pub(super) struct Credential<'a> {
    payload: &'a [u8],
}

impl<'a> Credential<'a> {
    pub(super) fn parse(payload: &'a [u8]) -> Result<Self, Failure> {
        if payload.len() != PAYLOAD_SIZE || !payload.starts_with(PAYLOAD) {
            return Err(refused(
                "the protected key payload has an unsupported size or version.",
            ));
        }
        Ok(Self { payload })
    }

    pub(super) fn receipt(&self, protection: &'static str) -> PublicDeviceKey {
        let (device_id, credential_id, public_key) = self.identity();
        PublicDeviceKey {
            device_id,
            credential_id,
            public_key,
            protection,
            status: "local_key_only",
        }
    }

    fn identity(&self) -> (String, String, String) {
        let offset = PAYLOAD.len();
        let device = u128::from_be_bytes(self.payload[offset..offset + 16].try_into().unwrap());
        let credential =
            u128::from_be_bytes(self.payload[offset + 16..offset + 32].try_into().unwrap());
        let signing = SigningKey::from_bytes(self.seed());
        let public_key = signing
            .verifying_key()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        (
            crate::id::encode(device),
            crate::id::encode(credential),
            public_key,
        )
    }

    pub(super) fn prove(&self, challenge: &Challenge, now: u64) -> Result<Proof, Failure> {
        challenge.validate(now)?;
        let (device_id, credential_id, public_key) = self.identity();
        if challenge.device_id != device_id
            || challenge.credential_id != credential_id
            || challenge.public_key != public_key
        {
            return Err(refused("the challenge does not match the protected key."));
        }
        Proof::signed(challenge.clone(), &SigningKey::from_bytes(self.seed()), now)
    }

    fn seed(&self) -> &[u8; 32] {
        self.payload[PAYLOAD_SIZE - 32..].try_into().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 8032, section 7.1, test 1 supplies the seed and public key.
    const PUBLIC_KEY: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    fn payload() -> [u8; 84] {
        let mut bytes = [0; 84];
        bytes[..20].copy_from_slice(b"vivac.device-key.v1\0");
        for (index, byte) in bytes[20..52].iter_mut().enumerate() {
            *byte = index as u8;
        }
        bytes[52..].copy_from_slice(&[
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
            0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
            0x1c, 0xae, 0x7f, 0x60,
        ]);
        bytes
    }

    fn challenge() -> Challenge {
        Challenge {
            version: 1,
            organization_id: "00000000000000000000000001".into(),
            principal_id: "00000000000000000000000002".into(),
            device_id: "00041061050r3gg28a1c60t3gf".into(),
            credential_id: "0g2491650n2rbhg68t3ce1t7gz".into(),
            public_key: PUBLIC_KEY.into(),
            operation_id: "00000000000000000000000005".into(),
            base_revision: 0,
            nonce: "ab".repeat(32),
            issued_at: 1000,
            expires_at: 1300,
        }
    }

    #[test]
    fn payload_requires_exact_size_and_every_magic_byte() {
        let bytes = payload();
        for cut in 0..bytes.len() {
            assert!(Credential::parse(&bytes[..cut]).is_err(), "cut {cut}");
        }
        let mut longer = bytes.to_vec();
        longer.push(0);
        assert!(Credential::parse(&longer).is_err());
        for index in 0..20 {
            let mut changed = bytes;
            changed[index] ^= 1;
            assert!(Credential::parse(&changed).is_err(), "magic byte {index}");
        }
    }

    #[test]
    fn independent_payload_vector_preserves_public_receipt() {
        let bytes = payload();
        let receipt = Credential::parse(&bytes)
            .unwrap()
            .receipt("windows_current_user_dpapi");
        assert_eq!(
            serde_json::to_value(receipt).unwrap(),
            serde_json::json!({
                "device_id": "00041061050r3gg28a1c60t3gf",
                "credential_id": "0g2491650n2rbhg68t3ce1t7gz",
                "public_key": PUBLIC_KEY,
                "protection": "windows_current_user_dpapi",
                "status": "local_key_only"
            })
        );
    }

    #[test]
    fn possession_proof_binds_the_entire_credential_identity() {
        let bytes = payload();
        let credential = Credential::parse(&bytes).unwrap();
        let original = challenge();
        for now in [1000, 1299] {
            credential
                .prove(&original, now)
                .unwrap()
                .verify(now)
                .unwrap();
        }
        for field in ["device_id", "credential_id", "public_key"] {
            let mut changed = original.clone();
            match field {
                "device_id" => changed.device_id = "00000000000000000000000003".into(),
                "credential_id" => changed.credential_id = "00000000000000000000000004".into(),
                // A different valid Ed25519 key (RFC 8032, section 7.1, test 2).
                _ => {
                    changed.public_key =
                        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c".into()
                }
            }
            assert!(changed.validate(1000).is_ok());
            assert!(
                matches!(credential.prove(&changed, 1000), Err(Failure::Usage(message))
                if message == "  Device key refused: the challenge does not match the protected key.")
            );
        }
    }

    #[test]
    fn signing_refuses_invalid_time_windows_and_noncanonical_fields() {
        let bytes = payload();
        let credential = Credential::parse(&bytes).unwrap();
        for now in [999, 1300, 1301] {
            assert!(credential.prove(&challenge(), now).is_err());
        }
        for (issued_at, expires_at, now) in [
            (1000, 1000, 1000),
            (1001, 1000, 1000),
            (1000, 1301, 1000),
            (u64::MAX, 0, u64::MAX),
            (0, u64::MAX, 0),
        ] {
            let mut changed = challenge();
            changed.issued_at = issued_at;
            changed.expires_at = expires_at;
            assert!(credential.prove(&changed, now).is_err());
        }
        let mut changed = challenge();
        changed.device_id = "80000000000000000000000000".into();
        assert!(credential.prove(&changed, 1000).is_err());
        changed = challenge();
        changed.nonce = "AB".repeat(32);
        assert!(credential.prove(&changed, 1000).is_err());
    }
}
