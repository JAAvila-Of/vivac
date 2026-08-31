//! Tiempo sin dependencias.
//!
//! El pilar de rendimiento pone la escritura de un nodo en p99 < 5 ms, y el de
//! seguridad quiere pocas dependencias que auditar. Formatear una fecha no
//! justifica ninguna de las dos cosas: son treinta lineas de aritmetica.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milisegundos desde epoch. Es lo que va dentro del ULID.
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Instante en UTC, RFC 3339 con segundos. Es lo que va en el evento.
pub fn now_rfc3339() -> String {
    let secs = unix_millis() / 1000;
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Los diez primeros caracteres de un RFC 3339, para presentacion.
pub fn date_of(ts: &str) -> &str {
    if ts.len() >= 10 {
        &ts[..10]
    } else {
        ts
    }
}

/// Algoritmo de Howard Hinnant: dias desde epoch a fecha civil proleptica
/// gregoriana. Vale para cualquier fecha, no solo para el rango de 32 bits.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_es_1970() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn fechas_conocidas() {
        // 2026-08-31 son 20696 dias desde epoch.
        assert_eq!(civil_from_days(20_696), (2026, 8, 31));
        // Un 29 de febrero, que es donde se rompen las implementaciones ingenuas.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        // Antes de epoch: el signo tiene que ir por la rama negativa.
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn formato_estable() {
        let s = now_rfc3339();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
    }
}
