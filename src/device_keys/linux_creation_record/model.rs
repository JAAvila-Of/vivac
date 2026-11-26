use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use std::io;

pub(super) const MAX_RECORD: usize = 2048;

pub(super) fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}

pub(super) fn require(value: bool) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn identifier(value: &str) -> bool {
    value.len() == 26
        && matches!(value.as_bytes()[0], b'0'..=b'7')
        && value
            .bytes()
            .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&byte))
}

fn hexadecimal(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Public record data only; validity supplies no producer evidence or authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub version: u64,
    pub installation_id: String,
    pub creation_id: String,
    pub device_id: String,
    pub credential_id: String,
    pub public_key: String,
    pub ciphertext_sha256: String,
    pub manifest_sha256: String,
    pub ciphertext_len: u64,
    pub manifest_len: u64,
    pub creation_profile_revision: u64,
    pub secret_policy_revision: u64,
}

impl Record {
    fn validate(&self) -> io::Result<()> {
        require(
            self.version == 2
                && self.creation_profile_revision == 1
                && self.secret_policy_revision == 1,
        )?;
        for value in [
            &self.installation_id,
            &self.creation_id,
            &self.device_id,
            &self.credential_id,
        ] {
            require(identifier(value))?;
        }
        require(
            hexadecimal(&self.public_key)
                && hexadecimal(&self.ciphertext_sha256)
                && hexadecimal(&self.manifest_sha256),
        )?;
        require(
            (1..=65536).contains(&self.ciphertext_len) && (1..=2048).contains(&self.manifest_len),
        )?;
        let package_len = self
            .ciphertext_len
            .checked_add(self.manifest_len)
            .ok_or_else(invalid)?;
        require(package_len <= 67584)?;
        let mut public = [0; 32];
        for (output, pair) in public
            .iter_mut()
            .zip(self.public_key.as_bytes().as_chunks::<2>().0.iter())
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
        let key = VerifyingKey::from_bytes(&public).map_err(|_| invalid())?;
        require(!key.is_weak())
    }

    pub fn canonical_bytes(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        require(bytes.len() <= MAX_RECORD)?;
        Ok(bytes)
    }
}
