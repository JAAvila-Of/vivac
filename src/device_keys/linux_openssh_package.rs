use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ssh_key::{Algorithm, Cipher, Kdf, PrivateKey};

pub const PROFILE: &str = "openssh-ed25519-aes256gcm-bcrypt-v1";
pub const MAX_KEY: u64 = 65536;
pub const MAX_MANIFEST: u64 = 2048;
pub const MAX_PACKAGE: u64 = 67584;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidPackage;

impl std::fmt::Display for InvalidPackage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid OpenSSH package")
    }
}

impl std::error::Error for InvalidPackage {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub profile: String,
    pub device_id: String,
    pub credential_id: String,
    pub public_key: String,
    pub key_sha256: String,
    pub key_size: u64,
}

fn lower_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_identifier(value: &str) -> bool {
    value.len() == 26
        && matches!(value.as_bytes().first(), Some(b'0'..=b'7'))
        && value
            .bytes()
            .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&byte))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

impl Manifest {
    pub fn validate(&self) -> Result<(), InvalidPackage> {
        if self.version != 1
            || self.profile != PROFILE
            || !valid_identifier(&self.device_id)
            || !valid_identifier(&self.credential_id)
            || !lower_hex(&self.public_key)
            || !lower_hex(&self.key_sha256)
            || self.key_size == 0
            || self.key_size > MAX_KEY
        {
            return Err(InvalidPackage);
        }
        Ok(())
    }
}

pub fn header_public_key(encoded: &[u8]) -> Result<String, InvalidPackage> {
    if encoded.is_empty() || encoded.len() as u64 > MAX_KEY {
        return Err(InvalidPackage);
    }
    let key = PrivateKey::from_openssh(encoded).map_err(|_| InvalidPackage)?;
    if !key.is_encrypted()
        || key.cipher() != Cipher::Aes256Gcm
        || key.algorithm() != Algorithm::Ed25519
        || !matches!(key.kdf(), Kdf::Bcrypt { salt, rounds } if salt.len() == 16 && *rounds == 64)
    {
        return Err(InvalidPackage);
    }
    let public = key
        .public_key()
        .key_data()
        .ed25519()
        .ok_or(InvalidPackage)?;
    Ok(hex(&public.0))
}

pub fn validate_key_bytes(key: &[u8], manifest: &Manifest) -> Result<(), InvalidPackage> {
    if key.is_empty()
        || key.len() as u64 > MAX_KEY
        || manifest.key_size != key.len() as u64
        || manifest.key_sha256 != hex(&Sha256::digest(key))
        || manifest.public_key != header_public_key(key)?
    {
        return Err(InvalidPackage);
    }
    Ok(())
}

// The writer retains phased reads; the resident reader consumes the byte pair.
#[allow(dead_code)]
pub fn validate_bytes(key: &[u8], manifest: &[u8]) -> Result<Manifest, InvalidPackage> {
    if key.is_empty()
        || key.len() as u64 > MAX_KEY
        || manifest.is_empty()
        || manifest.len() as u64 > MAX_MANIFEST
        || key.len() as u64 + manifest.len() as u64 > MAX_PACKAGE
    {
        return Err(InvalidPackage);
    }
    let manifest: Manifest = serde_json::from_slice(manifest).map_err(|_| InvalidPackage)?;
    manifest.validate()?;
    validate_key_bytes(key, &manifest)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = include_bytes!("../../tests/data/linux-resident/key.openssh");
    const MANIFEST: &[u8] = include_bytes!("../../tests/data/linux-resident/manifest.json");

    #[test]
    fn valid_encrypted_package_matches_its_manifest() {
        let expected: Manifest = serde_json::from_slice(MANIFEST).expect("fixture parsing failed");
        expected.validate().expect("fixture manifest rejected");
        assert_eq!(validate_key_bytes(KEY, &expected), Ok(()));
        assert_eq!(validate_bytes(KEY, MANIFEST), Ok(expected));
    }

    #[test]
    fn package_refuses_non_profile_rounds_with_matching_manifest() {
        let baseline = PrivateKey::from_openssh(KEY).expect("fixture key parsing failed");
        let bytes = baseline.to_bytes().expect("fixture binary encoding failed");
        let mut header = bytes
            .strip_prefix(b"openssh-key-v1\0")
            .expect("fixture magic differs");
        // Skip the SSH strings for the cipher and KDF names.
        for _ in 0..2 {
            let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
            header = &header[4 + length..];
        }
        // The option string contains the length-prefixed 16-byte salt and a u32.
        assert_eq!(&header[..8], &[0, 0, 0, 24, 0, 0, 0, 16]);
        let rounds_at = bytes.len() - header.len() + 24;
        assert_eq!(&bytes[rounds_at..rounds_at + 4], &64u32.to_be_bytes());
        let mut manifest: Manifest =
            serde_json::from_slice(MANIFEST).expect("invalid fixture manifest");
        assert_eq!(validate_key_bytes(KEY, &manifest), Ok(()));

        for rounds in [0u32, 1, 16, 63, 65, u32::MAX] {
            let mut changed = bytes.clone();
            changed[rounds_at..rounds_at + 4].copy_from_slice(&rounds.to_be_bytes());
            let changed = PrivateKey::from_bytes(&changed).expect("fixture framing changed");
            assert_eq!(changed.public_key(), baseline.public_key());
            assert!(
                matches!(changed.kdf(), Kdf::Bcrypt { rounds: observed, .. } if *observed == rounds)
            );
            // Re-encode the encrypted bytes without invoking a KDF.
            let changed = changed
                .to_openssh(ssh_key::LineEnding::LF)
                .expect("fixture encoding failed");
            manifest.key_size = changed.len() as u64;
            manifest.key_sha256 = hex(&Sha256::digest(changed.as_bytes()));
            let manifest_bytes =
                serde_json::to_vec(&manifest).expect("fixture serialization failed");
            assert_eq!(
                validate_bytes(changed.as_bytes(), &manifest_bytes),
                Err(InvalidPackage)
            );
        }
    }

    #[test]
    fn package_rejects_manifest_disagreement_and_changed_key_bytes() {
        let original: Manifest = serde_json::from_slice(MANIFEST).expect("fixture parsing failed");
        let mut variants = Vec::new();
        let mut changed = original.clone();
        changed.key_size += 1;
        variants.push(changed);
        let mut changed = original.clone();
        changed.key_sha256.replace_range(
            ..1,
            if original.key_sha256.starts_with('0') {
                "1"
            } else {
                "0"
            },
        );
        variants.push(changed);
        let mut changed = original.clone();
        changed.public_key.replace_range(
            ..1,
            if original.public_key.starts_with('0') {
                "1"
            } else {
                "0"
            },
        );
        variants.push(changed);
        for manifest in variants {
            assert!(validate_key_bytes(KEY, &manifest).is_err());
            let encoded = serde_json::to_vec(&manifest).expect("fixture serialization failed");
            assert!(validate_bytes(KEY, &encoded).is_err());
        }
        let mut changed_key = KEY.to_vec();
        changed_key[0] ^= 1;
        assert!(validate_bytes(&changed_key, MANIFEST).is_err());
        for extra in [",\"version\":1}", ",\"unknown\":false}"] {
            let encoded = std::str::from_utf8(MANIFEST)
                .expect("fixture encoding failed")
                .trim();
            let changed = format!("{}{}", &encoded[..encoded.len() - 1], extra);
            assert!(validate_bytes(KEY, changed.as_bytes()).is_err());
        }
        let mut padded = MANIFEST.to_vec();
        padded.resize(MAX_MANIFEST as usize, b' ');
        assert_eq!(validate_bytes(KEY, &padded), Ok(original));
        padded.push(b' ');
        assert!(validate_bytes(KEY, &padded).is_err());
    }
    fn manifest() -> Manifest {
        Manifest {
            version: 1,
            profile: PROFILE.to_owned(),
            device_id: "00000000000000000000000001".to_owned(),
            credential_id: "00000000000000000000000002".to_owned(),
            public_key: "a".repeat(64),
            key_sha256: "b".repeat(64),
            key_size: 500,
        }
    }

    #[test]
    fn manifest_rejects_malformed_unknown_duplicate_and_missing_fields() {
        let encoded = serde_json::to_string(&manifest()).expect("fixture serialization failed");
        let parsed: Manifest = serde_json::from_str(&encoded).expect("fixture parsing failed");
        assert!(parsed.validate().is_ok());
        for invalid in ["", "null", "[]", "{}", "{\"version\":"] {
            assert!(serde_json::from_str::<Manifest>(invalid).is_err());
        }
        for extra in [",\"version\":1}", ",\"unknown\":false}"] {
            let changed = format!("{}{}", &encoded[..encoded.len() - 1], extra);
            assert!(serde_json::from_str::<Manifest>(&changed).is_err());
        }
    }

    #[test]
    fn manifest_requires_canonical_profile_identifiers_hex_and_sizes() {
        let original = manifest();
        let mut variants = Vec::new();
        let mut changed = original.clone();
        changed.version = 2;
        variants.push(changed);
        let mut changed = original.clone();
        changed.profile.push(' ');
        variants.push(changed);
        for identifier in [
            "80000000000000000000000001",
            "0000000000000000000000000i",
            "short",
        ] {
            let mut changed = original.clone();
            changed.device_id = identifier.to_owned();
            variants.push(changed);
            let mut changed = original.clone();
            changed.credential_id = identifier.to_owned();
            variants.push(changed);
        }
        for invalid in ["A".repeat(64), "g".repeat(64), "a".repeat(63)] {
            let mut changed = original.clone();
            changed.public_key = invalid.clone();
            variants.push(changed);
            let mut changed = original.clone();
            changed.key_sha256 = invalid;
            variants.push(changed);
        }
        for size in [0, MAX_KEY + 1] {
            let mut changed = original.clone();
            changed.key_size = size;
            variants.push(changed);
        }
        assert!(variants.iter().all(|value| value.validate().is_err()));
        for size in [1, MAX_KEY] {
            let mut changed = original.clone();
            changed.key_size = size;
            assert!(changed.validate().is_ok());
        }
    }

    #[test]
    fn package_rejects_empty_and_oversized_members() {
        for (key, manifest) in [
            (Vec::new(), vec![b' '; 1]),
            (vec![b' '; 1], Vec::new()),
            (vec![b' '; MAX_KEY as usize + 1], vec![b' '; 1]),
            (vec![b' '; 1], vec![b' '; MAX_MANIFEST as usize + 1]),
            (
                vec![b' '; MAX_KEY as usize],
                vec![b' '; MAX_MANIFEST as usize + 1],
            ),
        ] {
            assert!(validate_bytes(&key, &manifest).is_err());
        }
    }
}
