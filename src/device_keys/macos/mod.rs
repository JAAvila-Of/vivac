use super::{refused, PublicDeviceKey};
use crate::failure::Failure;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
mod store;
#[cfg(target_os = "macos")]
pub(super) use store::{create, lock, prove, show, unlock};

pub(super) const PROTECTION: &str = "macos_classic_keychain_passphrase";
const MAX_MANIFEST_SIZE: usize = 2048;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u64,
    device_id: String,
    credential_id: String,
    public_key: String,
    database_digest: String,
}

impl Manifest {
    fn parse(bytes: &[u8]) -> Result<Self, Failure> {
        if bytes.len() > MAX_MANIFEST_SIZE {
            return Err(refused("the device key manifest exceeds its size limit."));
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|_| refused("the device key manifest is invalid."))?;
        if manifest.version != 1
            || !super::proof::canonical_identifier(&manifest.device_id)
            || !super::proof::canonical_identifier(&manifest.credential_id)
        {
            return Err(refused("the device key manifest identity is invalid."));
        }
        let public = super::proof::decode_hex::<32>(&manifest.public_key)?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&public)
            .map_err(|_| refused("the device key manifest public key is invalid."))?;
        if key.is_weak() {
            return Err(refused("the device key manifest public key is weak."));
        }
        super::proof::decode_hex::<32>(&manifest.database_digest)?;
        Ok(manifest)
    }

    fn from_receipt(receipt: &PublicDeviceKey, database_digest: String) -> Self {
        Self {
            version: 1,
            device_id: receipt.device_id.clone(),
            credential_id: receipt.credential_id.clone(),
            public_key: receipt.public_key.clone(),
            database_digest,
        }
    }

    fn matches_credential(&self, receipt: &PublicDeviceKey) -> Result<(), Failure> {
        if self.device_id != receipt.device_id
            || self.credential_id != receipt.credential_id
            || self.public_key != receipt.public_key
        {
            return Err(refused(
                "the device key manifest does not match the protected key.",
            ));
        }
        Ok(())
    }

    fn matches_database_digest(&self, digest: &[u8; 32]) -> Result<(), Failure> {
        if super::proof::decode_hex::<32>(&self.database_digest)? != *digest {
            return Err(refused(
                "the protected key database does not match its manifest digest.",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const VALID: &[u8] = br#"{"version":1,"device_id":"00000000000000000000000003","credential_id":"00000000000000000000000004","public_key":"d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a","database_digest":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}"#;

    #[test]
    fn manifest_requires_a_database_digest() {
        let mut without_digest: serde_json::Value = serde_json::from_slice(VALID).unwrap();
        without_digest
            .as_object_mut()
            .unwrap()
            .remove("database_digest");
        assert!(Manifest::parse(&serde_json::to_vec(&without_digest).unwrap()).is_err());
    }

    #[test]
    fn fixed_manifest_roundtrips_only_public_identity() {
        let manifest = Manifest::parse(VALID).unwrap();
        let receipt = PublicDeviceKey {
            device_id: "00000000000000000000000003".into(),
            credential_id: "00000000000000000000000004".into(),
            public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
            protection: PROTECTION,
            status: "local_key_only",
        };
        manifest.matches_credential(&receipt).unwrap();
        assert_eq!(
            serde_json::to_vec(&Manifest::from_receipt(
                &receipt,
                manifest.database_digest.clone()
            ))
            .unwrap(),
            VALID
        );
    }

    #[test]
    fn manifest_refuses_malformed_unknown_oversized_and_invalid_identity() {
        for bytes in [b"".as_slice(), b"null", b"[]", b"{}", b"{", b"{}{}"] {
            assert!(Manifest::parse(bytes).is_err());
        }
        let valid: serde_json::Value = serde_json::from_slice(VALID).unwrap();
        for (field, value) in [
            ("unknown", serde_json::json!(1)),
            ("version", serde_json::json!(0)),
            ("version", serde_json::json!(2)),
            ("version", serde_json::json!("1")),
            ("database_digest", serde_json::json!("")),
            ("database_digest", serde_json::json!("ab".repeat(31))),
            ("database_digest", serde_json::json!("gg".repeat(32))),
            ("database_digest", serde_json::json!("AB".repeat(32))),
            ("database_digest", serde_json::json!("ab".repeat(33))),
            ("device_id", serde_json::json!("80000000000000000000000000")),
            ("device_id", serde_json::json!("0000000000000000000000000I")),
            (
                "credential_id",
                serde_json::json!("0000000000000000000000000"),
            ),
            (
                "credential_id",
                serde_json::json!("0000000000000000000000000A"),
            ),
            ("public_key", serde_json::json!("00".repeat(32))),
            (
                "public_key",
                serde_json::json!("01".to_owned() + &"00".repeat(31)),
            ),
            // Edwards25519 has no curve point with encoded y = 2.
            (
                "public_key",
                serde_json::json!("02".to_owned() + &"00".repeat(31)),
            ),
            ("public_key", serde_json::json!("ff".repeat(31))),
            ("public_key", serde_json::json!("GG".repeat(32))),
            (
                "public_key",
                serde_json::json!(
                    "D75A980182B10AB7D54BFED3C964073A0EE172F3DAA62325AF021A68F707511A"
                ),
            ),
        ] {
            let mut changed = valid.clone();
            changed[field] = value;
            assert!(
                Manifest::parse(&serde_json::to_vec(&changed).unwrap()).is_err(),
                "{field}"
            );
        }
        let mut bounded = VALID.to_vec();
        bounded.resize(MAX_MANIFEST_SIZE, b' ');
        assert!(Manifest::parse(&bounded).is_ok());
        bounded.push(b' ');
        assert!(Manifest::parse(&bounded).is_err());
        let mut trailing = VALID.to_vec();
        trailing.push(b'x');
        assert!(Manifest::parse(&trailing).is_err());
    }

    #[test]
    fn manifest_matches_each_identity_field_without_authority() {
        let manifest = Manifest::parse(VALID).unwrap();
        for field in ["device_id", "credential_id", "public_key"] {
            let mut receipt = PublicDeviceKey {
                device_id: manifest.device_id.clone(),
                credential_id: manifest.credential_id.clone(),
                public_key: manifest.public_key.clone(),
                protection: PROTECTION,
                status: "local_key_only",
            };
            match field {
                "device_id" => receipt.device_id = "00000000000000000000000005".into(),
                "credential_id" => receipt.credential_id = "00000000000000000000000006".into(),
                _ => {
                    receipt.public_key =
                        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c".into()
                }
            }
            assert!(manifest.matches_credential(&receipt).is_err(), "{field}");
        }
    }

    #[test]
    fn persisted_digest_refuses_altered_database_bytes() {
        let manifest = Manifest::parse(VALID).unwrap();
        // The manifest carries the published SHA-256 vector for `abc`.
        let original: [u8; 32] = Sha256::digest(b"abc").into();
        manifest.matches_database_digest(&original).unwrap();
        for bytes in [b"abd".as_slice(), b"ab", b"abcd"] {
            let changed: [u8; 32] = Sha256::digest(bytes).into();
            assert!(manifest.matches_database_digest(&changed).is_err());
        }
    }
}
