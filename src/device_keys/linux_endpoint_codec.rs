use std::io;

const MAGIC: &[u8; 8] = b"VVEP01\0\0";
const DOMAIN: &[u8; 30] = b"vivac.linux.endpoint-proof.v1\0";

fn require(condition: bool) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::from(io::ErrorKind::InvalidData))
    }
}

fn nonzero(bytes: &[u8; 32]) -> io::Result<()> {
    require(bytes.iter().any(|byte| *byte != 0))
}

fn header(kind: u8, role: u8) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8] = 1;
    bytes[9] = kind;
    bytes[10] = role;
    bytes
}

fn check_header(bytes: &[u8], size: usize, kind: u8, role: u8) -> io::Result<()> {
    require(bytes.len() == size)?;
    require(bytes[..16] == header(kind, role))
}

#[derive(Debug, Eq, PartialEq)]
pub struct ChallengeFrame {
    pub nonce: [u8; 32],
}

impl ChallengeFrame {
    pub fn encode(&self) -> io::Result<[u8; 48]> {
        nonzero(&self.nonce)?;
        let mut bytes = [0; 48];
        bytes[..16].copy_from_slice(&header(1, 1));
        bytes[16..].copy_from_slice(&self.nonce);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        check_header(bytes, 48, 1, 1)?;
        let mut nonce = [0; 32];
        nonce.copy_from_slice(&bytes[16..]);
        nonzero(&nonce)?;
        Ok(Self { nonce })
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProofFrame {
    pub echo: [u8; 32],
    pub generation: [u8; 32],
    pub connection_nonce: [u8; 32],
    pub signature: [u8; 64],
}

impl ProofFrame {
    pub fn encode(&self) -> io::Result<[u8; 176]> {
        nonzero(&self.echo)?;
        nonzero(&self.generation)?;
        nonzero(&self.connection_nonce)?;
        let mut bytes = [0; 176];
        bytes[..16].copy_from_slice(&header(2, 2));
        bytes[16..48].copy_from_slice(&self.echo);
        bytes[48..80].copy_from_slice(&self.generation);
        bytes[80..112].copy_from_slice(&self.connection_nonce);
        bytes[112..].copy_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        check_header(bytes, 176, 2, 2)?;
        let mut frame = Self {
            echo: [0; 32],
            generation: [0; 32],
            connection_nonce: [0; 32],
            signature: [0; 64],
        };
        frame.echo.copy_from_slice(&bytes[16..48]);
        frame.generation.copy_from_slice(&bytes[48..80]);
        frame.connection_nonce.copy_from_slice(&bytes[80..112]);
        frame.signature.copy_from_slice(&bytes[112..]);
        nonzero(&frame.echo)?;
        nonzero(&frame.generation)?;
        nonzero(&frame.connection_nonce)?;
        Ok(frame)
    }
}

pub struct TranscriptData {
    pub record_digest: [u8; 32],
    pub store: [u64; 6],
    pub generation: [u8; 32],
    pub challenge: [u8; 32],
    pub connection_nonce: [u8; 32],
    pub verifier: (u32, u32, u32),
    pub custodian: (u32, u32, u32),
}

impl TranscriptData {
    pub fn encode(&self) -> io::Result<[u8; 238]> {
        nonzero(&self.generation)?;
        nonzero(&self.challenge)?;
        nonzero(&self.connection_nonce)?;
        for (pid, uid, gid) in [self.verifier, self.custodian] {
            require((1..=i32::MAX as u32).contains(&pid))?;
            require(uid != 0 && uid != u32::MAX && gid != 0 && gid != u32::MAX)?;
        }
        require(
            self.verifier.0 != self.custodian.0
                && self.verifier.1 == self.custodian.1
                && self.verifier.2 == self.custodian.2,
        )?;
        let mut bytes = [0; 238];
        bytes[..30].copy_from_slice(DOMAIN);
        bytes[30..32].copy_from_slice(&[1, 2]);
        bytes[38..70].copy_from_slice(&self.record_digest);
        for (word, output) in self.store.iter().zip(bytes[70..118].as_chunks_mut::<8>().0) {
            *output = word.to_be_bytes();
        }
        bytes[118..150].copy_from_slice(&self.generation);
        bytes[150..182].copy_from_slice(&self.challenge);
        bytes[182..214].copy_from_slice(&self.connection_nonce);
        let words = [
            self.verifier.0,
            self.verifier.1,
            self.verifier.2,
            self.custodian.0,
            self.custodian.1,
            self.custodian.2,
        ];
        for (word, output) in words.iter().zip(bytes[214..].as_chunks_mut::<4>().0) {
            *output = word.to_be_bytes();
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn proof() -> ProofFrame {
        ProofFrame {
            echo: [1; 32],
            generation: [2; 32],
            connection_nonce: [3; 32],
            signature: [0; 64],
        }
    }

    fn transcript() -> TranscriptData {
        TranscriptData {
            record_digest: [4; 32],
            store: [1, 2, 3, 4, 5, 6],
            generation: [2; 32],
            challenge: [1; 32],
            connection_nonce: [3; 32],
            verifier: (101, 1000, 1000),
            custodian: (202, 1000, 1000),
        }
    }

    #[test]
    fn endpoint_codec_matches_canonical_frame_hex_vectors() {
        let challenge = ChallengeFrame { nonce: [1; 32] };
        let challenge_bytes = challenge.encode().unwrap();
        assert_eq!(
            hex(&challenge_bytes),
            concat!(
                "56564550303100000101010000000000",
                "0101010101010101010101010101010101010101010101010101010101010101",
            )
        );
        assert_eq!(ChallengeFrame::decode(&challenge_bytes).unwrap(), challenge);
        let frame = proof();
        let proof_bytes = frame.encode().unwrap();
        assert_eq!(
            hex(&proof_bytes),
            concat!(
                "56564550303100000102020000000000",
                "0101010101010101010101010101010101010101010101010101010101010101",
                "0202020202020202020202020202020202020202020202020202020202020202",
                "0303030303030303030303030303030303030303030303030303030303030303",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
        );
        assert_eq!(ProofFrame::decode(&proof_bytes).unwrap(), frame);
    }

    #[test]
    fn endpoint_codec_matches_canonical_transcript_hex_vector() {
        assert_eq!(
            hex(&transcript().encode().unwrap()),
            concat!(
                "76697661632e6c696e75782e656e64706f696e742d70726f6f662e763100",
                "0102000000000000",
                "0404040404040404040404040404040404040404040404040404040404040404",
                "000000000000000100000000000000020000000000000003",
                "000000000000000400000000000000050000000000000006",
                "0202020202020202020202020202020202020202020202020202020202020202",
                "0101010101010101010101010101010101010101010101010101010101010101",
                "0303030303030303030303030303030303030303030303030303030303030303",
                "00000065000003e8000003e8",
                "000000ca000003e8000003e8",
            )
        );
    }

    #[test]
    fn endpoint_codec_binds_each_transcript_field_at_its_fixed_offset() {
        let original = transcript().encode().unwrap();
        for (field, range) in [(0, 38..70), (1, 118..150), (2, 150..182), (3, 182..214)] {
            let mut data = transcript();
            match field {
                0 => data.record_digest = [9; 32],
                1 => data.generation = [9; 32],
                2 => data.challenge = [9; 32],
                _ => data.connection_nonce = [9; 32],
            }
            let changed = data.encode().unwrap();
            assert_eq!(&changed[range.clone()], &[9; 32]);
            assert_eq!(&changed[..range.start], &original[..range.start]);
            assert_eq!(&changed[range.end..], &original[range.end..]);
        }
        for word in 0..6 {
            let mut data = transcript();
            data.store[word] = 0x0102030405060708;
            let changed = data.encode().unwrap();
            let start = 70 + 8 * word;
            assert_eq!(&changed[start..start + 8], &[1, 2, 3, 4, 5, 6, 7, 8]);
            assert_eq!(&changed[..start], &original[..start]);
            assert_eq!(&changed[start + 8..], &original[start + 8..]);
        }
        for field in 0..4 {
            let mut data = transcript();
            match field {
                0 => data.verifier.0 = 0x01020304,
                1 => data.custodian.0 = 0x01020304,
                2 => {
                    data.verifier.1 = 0x01020304;
                    data.custodian.1 = 0x01020304;
                }
                _ => {
                    data.verifier.2 = 0x01020304;
                    data.custodian.2 = 0x01020304;
                }
            }
            let changed = data.encode().unwrap();
            let starts: &[usize] = match field {
                0 => &[214],
                1 => &[226],
                2 => &[218, 230],
                _ => &[222, 234],
            };
            for start in starts {
                assert_eq!(&changed[*start..*start + 4], &[1, 2, 3, 4]);
            }
            for offset in 0..238 {
                if !starts
                    .iter()
                    .any(|start| (*start..*start + 4).contains(&offset))
                {
                    assert_eq!(changed[offset], original[offset]);
                }
            }
        }
    }

    #[test]
    fn endpoint_codec_accepts_data_boundaries_without_minting_authority() {
        let mut data = transcript();
        data.record_digest = [0; 32];
        data.store = [0, u64::MAX, 0, u64::MAX, 0, u64::MAX];
        data.verifier = (1, u32::MAX - 1, 1);
        data.custodian = (i32::MAX as u32, u32::MAX - 1, 1);
        let encoded = data.encode().unwrap();
        assert_eq!(&encoded[38..70], &[0; 32]);
        assert_eq!(&encoded[78..86], &u64::MAX.to_be_bytes());
        for offset in 0..32 {
            let mut nonce = [0; 32];
            nonce[offset] = 1;
            let frame = ChallengeFrame { nonce };
            assert_eq!(
                ChallengeFrame::decode(&frame.encode().unwrap()).unwrap(),
                frame
            );
        }
        let mut frame = proof();
        assert_eq!(
            ProofFrame::decode(&frame.encode().unwrap())
                .unwrap()
                .signature,
            [0; 64]
        );
        frame.signature = [0xff; 64];
        assert_eq!(ProofFrame::decode(&frame.encode().unwrap()).unwrap(), frame);
    }

    #[test]
    fn endpoint_codec_binds_every_frame_payload_byte_without_verifying_signatures() {
        let challenge = ChallengeFrame { nonce: [1; 32] }.encode().unwrap();
        for offset in 16..48 {
            let mut changed = challenge;
            changed[offset] ^= 0x80;
            let decoded = ChallengeFrame::decode(&changed).unwrap();
            assert_eq!(decoded.nonce[offset - 16], 0x81);
            assert_eq!(decoded.encode().unwrap(), changed);
        }
        let proof = proof().encode().unwrap();
        for offset in 16..176 {
            let mut changed = proof;
            changed[offset] ^= 0x80;
            let decoded = ProofFrame::decode(&changed).unwrap();
            assert_eq!(decoded.encode().unwrap(), changed);
        }
    }

    #[test]
    fn endpoint_codec_domain_is_refused_by_the_existing_enrollment_identity() {
        use super::super::linux_session::SigningIdentity;
        use ed25519_dalek::SigningKey;
        let public = SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes();
        let mut blob = Vec::new();
        for bytes in [b"ssh-ed25519".as_slice(), public.as_slice()] {
            blob.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            blob.extend_from_slice(bytes);
        }
        let comment = b"vivac.device-key.v1:endpoint-codec-fixture".to_vec();
        let enrollment = b"vivac.device.enrollment-proof.v1\0public challenge".to_vec();
        assert!(SigningIdentity::new(blob.clone(), public, enrollment, comment.clone()).is_ok());
        assert!(SigningIdentity::new(
            blob,
            public,
            transcript().encode().unwrap().to_vec(),
            comment
        )
        .is_err());
    }

    #[test]
    fn endpoint_codec_refuses_malformed_headers_and_lengths() {
        let challenge = ChallengeFrame { nonce: [1; 32] }.encode().unwrap();
        let proof = proof().encode().unwrap();
        for bytes in [challenge.as_slice(), proof.as_slice()] {
            for (offset, values) in [
                (8, &[0, 2, 255][..]),
                (9, &[0, 1, 2, 3][..]),
                (10, &[0, 1, 2, 3][..]),
            ] {
                for value in values {
                    if *value == bytes[offset] {
                        continue;
                    }
                    let mut changed = bytes.to_vec();
                    changed[offset] = *value;
                    assert!(if bytes.len() == 48 {
                        ChallengeFrame::decode(&changed).is_err()
                    } else {
                        ProofFrame::decode(&changed).is_err()
                    });
                }
            }
            for offset in 0..16 {
                let mut changed = bytes.to_vec();
                changed[offset] ^= 0x80;
                assert!(if bytes.len() == 48 {
                    ChallengeFrame::decode(&changed).is_err()
                } else {
                    ProofFrame::decode(&changed).is_err()
                });
            }
            for length in 0..bytes.len() {
                assert!(if bytes.len() == 48 {
                    ChallengeFrame::decode(&bytes[..length]).is_err()
                } else {
                    ProofFrame::decode(&bytes[..length]).is_err()
                });
            }
            let mut extended = bytes.to_vec();
            extended.push(0);
            assert!(if bytes.len() == 48 {
                ChallengeFrame::decode(&extended).is_err()
            } else {
                ProofFrame::decode(&extended).is_err()
            });
        }
    }

    #[test]
    fn endpoint_codec_refuses_zero_nonces_and_generation() {
        assert!(ChallengeFrame { nonce: [0; 32] }.encode().is_err());
        let mut challenge = ChallengeFrame { nonce: [1; 32] }.encode().unwrap();
        challenge[16..].fill(0);
        assert!(ChallengeFrame::decode(&challenge).is_err());
        for field in 0..3 {
            let mut encoded = proof().encode().unwrap();
            encoded[16 + field * 32..48 + field * 32].fill(0);
            assert!(ProofFrame::decode(&encoded).is_err());
            let mut frame = proof();
            match field {
                0 => frame.echo = [0; 32],
                1 => frame.generation = [0; 32],
                _ => frame.connection_nonce = [0; 32],
            }
            assert!(frame.encode().is_err());
            let mut data = transcript();
            match field {
                0 => data.challenge = [0; 32],
                1 => data.generation = [0; 32],
                _ => data.connection_nonce = [0; 32],
            }
            assert!(data.encode().is_err());
        }
    }

    #[test]
    fn endpoint_codec_refuses_invalid_peer_tuples() {
        for peer in [
            (0, 1000, 1000),
            (i32::MAX as u32 + 1, 1000, 1000),
            (101, 0, 1000),
            (101, u32::MAX, 1000),
            (101, 1000, 0),
            (101, 1000, u32::MAX),
        ] {
            let mut data = transcript();
            data.verifier = peer;
            assert!(data.encode().is_err());
            let mut data = transcript();
            data.custodian = peer;
            assert!(data.encode().is_err());
        }
        let mut data = transcript();
        data.custodian.0 = data.verifier.0;
        assert!(data.encode().is_err());
        let mut data = transcript();
        data.custodian.1 += 1;
        assert!(data.encode().is_err());
        let mut data = transcript();
        data.custodian.2 += 1;
        assert!(data.encode().is_err());
    }
}
