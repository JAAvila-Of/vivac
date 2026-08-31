//! ULID: 48 bits of time + 80 of randomness, in lowercase Crockford base32.
//!
//! `MODEL.md` §3.6 picks them for two reasons that still hold: they do not
//! collide across machines without coordination --which is what will be needed
//! the day `events` gets synced-- and in lowercase they match the AGM ID
//! pattern, so the projection translates nothing.
//!
//! Nobody sees them from outside: the user handles aliases (`t7`), and
//! `vivac why 7` works with the number alone.

const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A fresh ULID. The time prefix makes them sort by creation.
pub fn ulid() -> String {
    let ms = crate::clock::unix_millis() & 0xFFFF_FFFF_FFFF;
    let mut rand = [0u8; 10];
    if getrandom::getrandom(&mut rand).is_err() {
        // With no system randomness we do not invent randomness: degrade to
        // something unique within this machine and carry on. A repeated ID
        // breaks provenance, but aborting here breaks capture, which is worse.
        let n = ms.rotate_left(17) ^ (std::process::id() as u64);
        rand[..8].copy_from_slice(&n.to_be_bytes());
    }
    let mut v: u128 = (ms as u128) << 80;
    for (i, b) in rand.iter().enumerate() {
        v |= (*b as u128) << (8 * (9 - i));
    }
    encode(v)
}

fn encode(mut v: u128) -> String {
    let mut out = [0u8; 26];
    for i in (0..26).rev() {
        out[i] = CROCKFORD[(v & 0x1F) as usize];
        v >>= 5;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longitud_y_alfabeto() {
        let u = ulid();
        assert_eq!(u.len(), 26);
        assert!(u.bytes().all(|b| CROCKFORD.contains(&b)));
    }

    #[test]
    fn no_repite() {
        let a: std::collections::HashSet<String> = (0..1000).map(|_| ulid()).collect();
        assert_eq!(a.len(), 1000);
    }

    #[test]
    fn ordena_por_tiempo() {
        let a = ulid();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = ulid();
        assert!(a < b, "{a} should sort before {b}");
    }
}
