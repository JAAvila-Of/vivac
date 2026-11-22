use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, ()>;

pub(crate) const ENCODED_LENGTH: usize = 64;
const SOURCE_LENGTH: usize = 32;

fn generate_with(mut fill: impl FnMut(&mut [u8]) -> Result<()>) -> Result<Zeroizing<Vec<u8>>> {
    let mut source = Zeroizing::new([0u8; SOURCE_LENGTH]);
    fill(&mut *source)?;
    let mut encoded = Zeroizing::new(vec![0u8; ENCODED_LENGTH]);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (byte, pair) in source.iter().zip(encoded.chunks_exact_mut(2)) {
        pair[0] = HEX[usize::from(byte >> 4)];
        pair[1] = HEX[usize::from(byte & 0x0f)];
    }
    Ok(encoded)
}

pub(crate) fn generate() -> Result<Zeroizing<Vec<u8>>> {
    generate_with(|buffer| getrandom::getrandom(buffer).map_err(|_| ()))
}

pub(crate) fn confirmation_matches(secret: &[u8], confirmation: &[u8]) -> bool {
    secret.len() == ENCODED_LENGTH
        && secret
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        && secret == confirmation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_full_random_draw_is_encoded_without_losing_leading_zeroes() {
        let mut calls = 0;
        let secret = generate_with(|buffer| {
            calls += 1;
            assert_eq!(buffer.len(), SOURCE_LENGTH);
            for (position, byte) in buffer.iter_mut().enumerate() {
                *byte = position as u8;
            }
            buffer[31] = 0xff;
            Ok(())
        })
        .expect("deterministic source should succeed");
        assert_eq!(calls, 1);
        assert_eq!(
            secret.as_slice(),
            b"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1eff"
        );
    }

    #[test]
    fn random_source_failure_never_returns_an_encoded_partial_secret() {
        for partial in [false, true] {
            let mut calls = 0;
            let result = generate_with(|buffer| {
                calls += 1;
                if partial {
                    buffer[..16].fill(0xab);
                }
                Err(())
            });
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn confirmation_requires_the_exact_generated_encoding() {
        let generated = [b'a'; ENCODED_LENGTH];
        assert!(confirmation_matches(&generated, &generated));
        for invalid in [
            vec![b'a'; ENCODED_LENGTH - 1],
            vec![b'a'; ENCODED_LENGTH + 1],
            vec![b'A'; ENCODED_LENGTH],
            vec![b'b'; ENCODED_LENGTH],
            [generated.to_vec(), vec![b'\n']].concat(),
        ] {
            assert!(!confirmation_matches(&generated, &invalid));
        }
        assert!(!confirmation_matches(
            &[b'G'; ENCODED_LENGTH],
            &[b'G'; ENCODED_LENGTH]
        ));
        assert!(!confirmation_matches(b"", b""));
    }
}
