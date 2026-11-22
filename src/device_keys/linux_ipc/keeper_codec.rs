use sha2::{Digest, Sha256};
use std::io;

const TAG: &[u8; 8] = b"VVKEEP01";
const OFFER_DOMAIN: &[u8] = b"vivac.linux.keeper-offer.v1\0";

fn nonzero(field: &[u8; 32]) -> io::Result<()> {
    if field.iter().all(|byte| *byte == 0) {
        Err(io::ErrorKind::InvalidData.into())
    } else {
        Ok(())
    }
}

fn header<const N: usize>(kind: u8) -> [u8; N] {
    let mut bytes = [0; N];
    bytes[..8].copy_from_slice(TAG);
    bytes[8] = 1;
    bytes[9] = kind;
    bytes
}

fn validate_header(bytes: &[u8], length: usize, kind: u8) -> io::Result<()> {
    if bytes.len() != length || bytes[..16] != header::<16>(kind) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(())
}

fn field(bytes: &[u8]) -> io::Result<[u8; 32]> {
    let result = bytes
        .try_into()
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    nonzero(&result)?;
    Ok(result)
}

fn offer_digest(offer: &OfferFrame) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(OFFER_DOMAIN);
    hash.update(offer.encode());
    hash.finalize().into()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChallengeFrame {
    challenge: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferFrame {
    challenge: [u8; 32],
    admission_fingerprint: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmationFrame {
    challenge: [u8; 32],
    offer_digest: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveClosureFrame {
    challenge: [u8; 32],
    offer_digest: [u8; 32],
    generation: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveClosureConfirmationFrame(ActiveClosureFrame);

impl ChallengeFrame {
    pub fn new(challenge: [u8; 32]) -> io::Result<Self> {
        nonzero(&challenge)?;
        Ok(Self { challenge })
    }
    pub fn challenge(&self) -> &[u8; 32] {
        &self.challenge
    }
    pub fn encode(&self) -> [u8; 48] {
        let mut bytes = header::<48>(1);
        bytes[16..].copy_from_slice(&self.challenge);
        bytes
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        validate_header(bytes, 48, 1)?;
        Self::new(field(&bytes[16..])?)
    }
}

impl OfferFrame {
    pub fn new(challenge: [u8; 32], admission_fingerprint: [u8; 32]) -> io::Result<Self> {
        nonzero(&challenge)?;
        nonzero(&admission_fingerprint)?;
        Ok(Self {
            challenge,
            admission_fingerprint,
        })
    }
    pub fn encode(&self) -> [u8; 80] {
        let mut bytes = header::<80>(2);
        bytes[16..48].copy_from_slice(&self.challenge);
        bytes[48..].copy_from_slice(&self.admission_fingerprint);
        bytes
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        validate_header(bytes, 80, 2)?;
        Self::new(field(&bytes[16..48])?, field(&bytes[48..])?)
    }
    pub fn validate_expected(
        &self,
        challenge: &ChallengeFrame,
        admission_fingerprint: &[u8; 32],
    ) -> io::Result<()> {
        self.validate_challenge(challenge)?;
        if self.admission_fingerprint != *admission_fingerprint {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }

    pub(super) fn validate_challenge(&self, challenge: &ChallengeFrame) -> io::Result<()> {
        if self.challenge != challenge.challenge {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }
}

impl ConfirmationFrame {
    pub fn new(offer: &OfferFrame) -> io::Result<Self> {
        let offer_digest = offer_digest(offer);
        nonzero(&offer_digest)?;
        Ok(Self {
            challenge: offer.challenge,
            offer_digest,
        })
    }
    pub fn encode(&self) -> [u8; 80] {
        let mut bytes = header::<80>(3);
        bytes[16..48].copy_from_slice(&self.challenge);
        bytes[48..].copy_from_slice(&self.offer_digest);
        bytes
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        validate_header(bytes, 80, 3)?;
        Ok(Self {
            challenge: field(&bytes[16..48])?,
            offer_digest: field(&bytes[48..])?,
        })
    }
    pub fn validate_offer(&self, offer: &OfferFrame) -> io::Result<()> {
        if self.challenge != offer.challenge || self.offer_digest != offer_digest(offer) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }
}

impl ActiveClosureFrame {
    pub fn new(offer: &OfferFrame, generation: [u8; 32]) -> io::Result<Self> {
        nonzero(&generation)?;
        let offer_digest = offer_digest(offer);
        nonzero(&offer_digest)?;
        Ok(Self {
            challenge: offer.challenge,
            offer_digest,
            generation,
        })
    }

    pub fn encode(&self) -> [u8; 112] {
        let mut bytes = header::<112>(4);
        bytes[16..48].copy_from_slice(&self.challenge);
        bytes[48..80].copy_from_slice(&self.offer_digest);
        bytes[80..].copy_from_slice(&self.generation);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        validate_header(bytes, 112, 4)?;
        Ok(Self {
            challenge: field(&bytes[16..48])?,
            offer_digest: field(&bytes[48..80])?,
            generation: field(&bytes[80..112])?,
        })
    }

    pub fn validate_offer(&self, offer: &OfferFrame) -> io::Result<()> {
        if self.challenge != offer.challenge || self.offer_digest != offer_digest(offer) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }

    pub fn validate_generation(&self, generation: &[u8; 32]) -> io::Result<()> {
        if self.generation != *generation {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }
}

impl ActiveClosureConfirmationFrame {
    pub fn new(offer: &OfferFrame, generation: [u8; 32]) -> io::Result<Self> {
        ActiveClosureFrame::new(offer, generation).map(Self)
    }

    pub fn encode(&self) -> [u8; 112] {
        let mut bytes = self.0.encode();
        bytes[9] = 5;
        bytes
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        validate_header(bytes, 112, 5)?;
        Ok(Self(ActiveClosureFrame {
            challenge: field(&bytes[16..48])?,
            offer_digest: field(&bytes[48..80])?,
            generation: field(&bytes[80..112])?,
        }))
    }

    pub fn validate_offer(&self, offer: &OfferFrame) -> io::Result<()> {
        self.0.validate_offer(offer)
    }

    pub fn validate_generation(&self, generation: &[u8; 32]) -> io::Result<()> {
        self.0.validate_generation(generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(kind: u8) -> Vec<u8> {
        let mut bytes = b"VVKEEP01".to_vec();
        bytes.extend([1, kind, 0, 0, 0, 0, 0, 0]);
        bytes.extend(1..=32);
        if kind != 1 {
            bytes.extend(33..=64);
        }
        if kind == 4 || kind == 5 {
            bytes.extend(65..=96);
        }
        bytes
    }

    #[test]
    fn golden_frames_and_confirmation_transcript() {
        let challenge_bytes = vector(1);
        let offer_bytes = vector(2);
        let challenge = ChallengeFrame::decode(&challenge_bytes).unwrap();
        let offer = OfferFrame::decode(&offer_bytes).unwrap();
        assert_eq!(challenge.encode().as_slice(), challenge_bytes);
        assert_eq!(offer.encode().as_slice(), offer_bytes);
        assert_eq!(
            ChallengeFrame::new(std::array::from_fn(|i| i as u8 + 1)).unwrap(),
            challenge
        );
        assert_eq!(
            OfferFrame::new(
                std::array::from_fn(|i| i as u8 + 1),
                std::array::from_fn(|i| i as u8 + 33)
            )
            .unwrap(),
            offer
        );
        let expected_digest = "7bb0d2333f0be2f46740087bff722c6632b9061000cca66b1e4a525cd16eab01";
        let mut confirmation_bytes = vector(3);
        for (i, byte) in confirmation_bytes[48..].iter_mut().enumerate() {
            *byte = u8::from_str_radix(&expected_digest[i * 2..i * 2 + 2], 16).unwrap();
        }
        let confirmation = ConfirmationFrame::new(&offer).unwrap();
        assert_eq!(confirmation.encode().as_slice(), confirmation_bytes);
        assert_eq!(
            ConfirmationFrame::decode(&confirmation_bytes).unwrap(),
            confirmation
        );
        confirmation.validate_offer(&offer).unwrap();
    }

    fn accepts(kind: u8, bytes: &[u8]) -> bool {
        match kind {
            1 => ChallengeFrame::decode(bytes).is_ok(),
            2 => OfferFrame::decode(bytes).is_ok(),
            3 => ConfirmationFrame::decode(bytes).is_ok(),
            4 => ActiveClosureFrame::decode(bytes).is_ok(),
            5 => ActiveClosureConfirmationFrame::decode(bytes).is_ok(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn malformed_frames_are_rejected() {
        for kind in 1..=5 {
            let valid = vector(kind);
            assert!(accepts(kind, &valid));
            for length in 0..valid.len() {
                assert!(!accepts(kind, &valid[..length]));
            }
            let mut extended = valid.clone();
            extended.push(0);
            assert!(!accepts(kind, &extended));
            for index in 0..16 {
                let mut changed = valid.clone();
                changed[index] ^= 1;
                assert!(!accepts(kind, &changed), "kind {kind}, header {index}");
            }
            for start in (16..valid.len()).step_by(32) {
                let mut changed = valid.clone();
                changed[start..start + 32].fill(0);
                assert!(!accepts(kind, &changed), "kind {kind}, field {start}");
            }
        }
        assert!(ChallengeFrame::new([0; 32]).is_err());
        assert!(OfferFrame::new([0; 32], [1; 32]).is_err());
        assert!(OfferFrame::new([1; 32], [0; 32]).is_err());
        assert!(
            ActiveClosureFrame::new(&OfferFrame::new([1; 32], [2; 32]).unwrap(), [0; 32]).is_err()
        );
        assert!(ActiveClosureConfirmationFrame::new(
            &OfferFrame::new([1; 32], [2; 32]).unwrap(),
            [0; 32]
        )
        .is_err());
    }

    #[test]
    fn expected_offer_and_current_confirmation_are_required() {
        let challenge = ChallengeFrame::new([1; 32]).unwrap();
        let other_challenge = ChallengeFrame::new([2; 32]).unwrap();
        let offer = OfferFrame::new([1; 32], [3; 32]).unwrap();
        offer.validate_expected(&challenge, &[3; 32]).unwrap();
        assert!(offer.validate_expected(&other_challenge, &[3; 32]).is_err());
        assert!(offer.validate_expected(&challenge, &[4; 32]).is_err());
        let confirmation = ConfirmationFrame::new(&offer).unwrap();
        confirmation.validate_offer(&offer).unwrap();
        assert!(confirmation
            .validate_offer(&OfferFrame::new([2; 32], [3; 32]).unwrap())
            .is_err());
        assert!(confirmation
            .validate_offer(&OfferFrame::new([1; 32], [4; 32]).unwrap())
            .is_err());
        let mut changed = confirmation.encode();
        changed[16] ^= 1;
        assert!(ConfirmationFrame::decode(&changed)
            .unwrap()
            .validate_offer(&offer)
            .is_err());
        let mut changed = confirmation.encode();
        changed[48] ^= 1;
        assert!(ConfirmationFrame::decode(&changed)
            .unwrap()
            .validate_offer(&offer)
            .is_err());
    }

    #[test]
    fn active_closure_binds_exact_offer_and_generation() {
        let offer = OfferFrame::new([1; 32], [2; 32]).unwrap();
        let closure = ActiveClosureFrame::new(&offer, [3; 32]).unwrap();
        let decoded = ActiveClosureFrame::decode(&closure.encode()).unwrap();
        assert_eq!(decoded, closure);
        decoded.validate_offer(&offer).unwrap();
        decoded.validate_generation(&[3; 32]).unwrap();
        assert!(decoded
            .validate_offer(&OfferFrame::new([1; 32], [4; 32]).unwrap())
            .is_err());
        assert!(decoded.validate_generation(&[5; 32]).is_err());
        let mut changed = closure.encode();
        changed[80] ^= 1;
        assert!(ActiveClosureFrame::decode(&changed)
            .unwrap()
            .validate_generation(&[3; 32])
            .is_err());
    }

    #[test]
    fn active_closure_confirmation_requires_its_own_kind_and_exact_binding() {
        let offer = OfferFrame::new([1; 32], [2; 32]).unwrap();
        let confirmation = ActiveClosureConfirmationFrame::new(&offer, [3; 32]).unwrap();
        let encoded = confirmation.encode();
        assert_eq!(encoded.len(), 112);
        assert_eq!(encoded[9], 5);
        let decoded = ActiveClosureConfirmationFrame::decode(&encoded).unwrap();
        assert_eq!(decoded, confirmation);
        decoded.validate_offer(&offer).unwrap();
        decoded.validate_generation(&[3; 32]).unwrap();
        assert!(decoded.validate_generation(&[4; 32]).is_err());
        assert!(decoded
            .validate_offer(&OfferFrame::new([1; 32], [4; 32]).unwrap())
            .is_err());
        assert!(ActiveClosureFrame::decode(&encoded).is_err());
        assert!(ActiveClosureConfirmationFrame::decode(
            &ActiveClosureFrame::new(&offer, [3; 32]).unwrap().encode()
        )
        .is_err());
    }
}
