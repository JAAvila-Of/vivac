//! ULID: 48 bits de tiempo + 80 de azar, en base32 Crockford minuscula.
//!
//! `MODEL.md` §3.6 los elige por dos razones que siguen valiendo: no colisionan
//! entre maquinas sin coordinacion --lo que hace falta el dia que `events` se
//! sincronice-- y en minuscula cumplen el patron de IDs de AGM, asi que la
//! proyeccion no traduce nada.
//!
//! Hacia fuera nadie los ve: el usuario maneja alias (`t7`), y `vivac why 7`
//! funciona solo con el numero.

const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Un ULID nuevo. El prefijo de tiempo hace que ordenen por creacion.
pub fn ulid() -> String {
    let ms = crate::clock::unix_millis() & 0xFFFF_FFFF_FFFF;
    let mut rand = [0u8; 10];
    if getrandom::getrandom(&mut rand).is_err() {
        // Sin azar del sistema no se inventa azar: se degrada a algo unico
        // dentro de esta maquina y se sigue. Un ID repetido rompe la
        // procedencia, pero abortar aqui rompe la captura, que es peor.
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
        assert!(a < b, "{a} deberia ordenar antes que {b}");
    }
}
