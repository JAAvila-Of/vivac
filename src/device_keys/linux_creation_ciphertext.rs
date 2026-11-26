use ed25519_dalek::SigningKey;
use ssh_key::{
    private::{Ed25519Keypair, Ed25519PrivateKey, KeypairData},
    public::Ed25519PublicKey,
    Cipher, Kdf, LineEnding, PrivateKey,
};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, ()>;

pub(crate) const ROUNDS: u32 = 64;

pub(crate) fn valid_identifier(value: &str) -> bool {
    value.len() == 26
        && matches!(value.as_bytes().first(), Some(b'0'..=b'7'))
        && value
            .bytes()
            .all(|byte| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&byte))
}

pub(crate) fn validate_comment(value: &str) -> Result<()> {
    let identifiers = value.strip_prefix("vivac.device-key.v1:").ok_or(())?;
    let (device, credential) = identifiers.split_once(':').ok_or(())?;
    if valid_identifier(device) && valid_identifier(credential) {
        Ok(())
    } else {
        Err(())
    }
}

pub(crate) fn serialize_encrypted(comment: &str, password: &[u8]) -> Result<Zeroizing<String>> {
    let mut seed = Zeroizing::new([0u8; 32]);
    let mut salt = Zeroizing::new([0u8; 16]);
    let mut check_bytes = Zeroizing::new([0u8; 4]);
    getrandom::getrandom(&mut *seed).map_err(|_| ())?;
    getrandom::getrandom(&mut *salt).map_err(|_| ())?;
    getrandom::getrandom(&mut *check_bytes).map_err(|_| ())?;

    let signing = SigningKey::from_bytes(&seed);
    let pair = Ed25519Keypair {
        private: Ed25519PrivateKey::from_bytes(&seed),
        public: Ed25519PublicKey(signing.verifying_key().to_bytes()),
    };
    let private = PrivateKey::new(KeypairData::Ed25519(pair), comment).map_err(|_| ())?;
    let encrypted = private
        .encrypt_with(
            Cipher::Aes256Gcm,
            Kdf::Bcrypt {
                salt: salt.to_vec(),
                rounds: ROUNDS,
            },
            u32::from_be_bytes(*check_bytes),
            password,
        )
        .map_err(|_| ())?;
    if !encrypted.is_encrypted()
        || encrypted.cipher() != Cipher::Aes256Gcm
        || !matches!(encrypted.kdf(), Kdf::Bcrypt { salt, rounds } if salt.len() == 16 && *rounds == ROUNDS)
    {
        return Err(());
    }

    // ssh-key uses an internal Vec while encrypting. These explicit buffers
    // are zeroized, but this experiment does not promise total memory erasure.
    encrypted.to_openssh(LineEnding::LF).map_err(|_| ())
}

#[cfg(target_os = "linux")]
pub(crate) struct AttendedCiphertext {
    confirmation: super::creation_input::ConfirmedGeneratedSecret,
    encoded: Zeroizing<String>,
}

#[cfg(target_os = "linux")]
impl AttendedCiphertext {
    pub(crate) fn read(comment: &str) -> Result<Self> {
        validate_comment(comment)?;
        let confirmation = super::creation_input::read_generated_confirmed()?;
        let encoded = serialize_encrypted(comment, confirmation.as_bytes())?;
        Ok(Self {
            confirmation,
            encoded,
        })
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        super::creation_input::ConfirmedGeneratedSecret,
        Zeroizing<String>,
    ) {
        (self.confirmation, self.encoded)
    }
}
